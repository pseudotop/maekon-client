//! Inline unit tests for `sqlite::metrics`.
//!
//! Test harness convention per the Phase 5-D8 spec
//! (`docs/reviews/2026-04-18-phase5-d8-storage-test-backfill-spec.md`):
//! each test module defines its own `open_db()` locally. Do NOT centralize
//! this helper into `test_utils.rs` — that is an explicit follow-up item.

// `open_db()` is defined for future Err-branch tests that need a raw
// Connection handle, but is currently unused because every active
// test uses `open_storage()` + `storage.connection_arc()` instead.
// Keep the helper available for the follow-up phase.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Timelike, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::activity::{ProcessSnapshot, ProcessSnapshotEntry, SessionStats};
use oneshim_core::models::system::{NetworkInfo, SystemMetrics};
use oneshim_core::ports::storage::MetricsStorage;
use rusqlite::Connection;
use tempfile::TempDir;

use crate::sqlite::SqliteStorage;

// ── Harness ─────────────────────────────────────────────────────

/// Opens a fresh on-disk SQLite DB with all migrations applied.
/// Returned `TempDir` must outlive the test; drop order matters
/// (connection must drop before tempdir).
#[allow(dead_code)]
fn open_db() -> (TempDir, Arc<Mutex<Connection>>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let conn = Connection::open(dir.path().join("t.db")).expect("open sqlite");
    crate::migration::run_migrations(&conn).expect("run_migrations");
    (dir, Arc::new(Mutex::new(conn)))
}

/// Opens an in-memory `SqliteStorage` with the standard 30-day retention.
/// Used by tests that need the full `SqliteStorage` API (port methods).
fn open_storage() -> SqliteStorage {
    SqliteStorage::open_in_memory(30).expect("in-memory storage")
}

/// Truncate `Utc::now()` to the start of its own hour.
/// Used so tests never rely on hardcoded calendar dates.
fn current_hour_start() -> DateTime<Utc> {
    Utc::now()
        .with_minute(0)
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .expect("truncation to hour should always succeed")
}

// ── Sample builders ─────────────────────────────────────────────

fn sample_metrics(ts: DateTime<Utc>, cpu: f32) -> SystemMetrics {
    SystemMetrics {
        timestamp: ts,
        cpu_usage: cpu,
        memory_used: 8_000_000_000,
        memory_total: 16_000_000_000,
        disk_used: 100_000_000_000,
        disk_total: 500_000_000_000,
        network: Some(NetworkInfo {
            upload_speed: 1_000,
            download_speed: 2_000,
            is_connected: true,
        }),
        typing_wpm: 0.0,
    }
}

fn sample_metrics_no_network(ts: DateTime<Utc>, cpu: f32) -> SystemMetrics {
    let mut m = sample_metrics(ts, cpu);
    m.network = None;
    m
}

fn sample_process_snapshot(ts: DateTime<Utc>, count: usize) -> ProcessSnapshot {
    ProcessSnapshot {
        timestamp: ts,
        processes: (0..count)
            .map(|i| ProcessSnapshotEntry {
                pid: i as u32,
                name: format!("proc-{i}"),
                cpu_usage: 1.0 + (i as f32),
                memory_bytes: (100 + i as u64) * 1_048_576,
            })
            .collect(),
    }
}

fn sample_session(id: &str, events: u64, frames: u64) -> SessionStats {
    SessionStats {
        session_id: id.to_string(),
        started_at: Utc::now(),
        ended_at: None,
        total_events: events,
        total_frames: frames,
        total_idle_secs: 0,
    }
}

// ── aggregate_hourly_metrics ──────────────────────────────────

#[tokio::test]
async fn aggregate_hourly_metrics_rolls_up_samples_in_hour() {
    let storage = open_storage();
    let hour_start = current_hour_start();
    let hour_key = hour_start.format("%Y-%m-%dT%H:00:00Z").to_string();

    // Seed 3 samples in the current hour with known CPU values.
    for (offset_min, cpu) in [(5, 20.0_f32), (20, 60.0_f32), (50, 40.0_f32)] {
        let ts = hour_start + Duration::minutes(offset_min);
        storage
            .save_metrics(&sample_metrics(ts, cpu))
            .await
            .unwrap();
    }

    storage.aggregate_hourly_metrics(hour_start).await.unwrap();

    let rows = storage.list_hourly_metrics_since(&hour_key).unwrap();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.hour, hour_key);
    assert!(
        (r.cpu_avg - 40.0_f64).abs() < 0.1,
        "cpu_avg should be ~40.0, got {}",
        r.cpu_avg
    );
    assert!(
        (r.cpu_max - 60.0_f64).abs() < 0.1,
        "cpu_max should be 60.0, got {}",
        r.cpu_max
    );
    assert_eq!(r.sample_count, 3);
}

#[tokio::test]
async fn aggregate_hourly_metrics_empty_hour_writes_no_row() {
    let storage = open_storage();
    // Pick 6 hours in the future so no sample could possibly be there.
    let hour_start = current_hour_start() + Duration::hours(6);
    let hour_key = hour_start.format("%Y-%m-%dT%H:00:00Z").to_string();

    storage.aggregate_hourly_metrics(hour_start).await.unwrap();

    let rows = storage.list_hourly_metrics_since(&hour_key).unwrap();
    assert!(
        rows.is_empty(),
        "empty hour must not produce an aggregate row"
    );
}

#[tokio::test]
async fn aggregate_hourly_metrics_utc_midnight_boundary() {
    let storage = open_storage();
    // Anchor to "today's UTC midnight" regardless of when the test runs.
    let day_midnight: DateTime<Utc> = Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();

    // 23:00 bucket of yesterday and 00:00 bucket of today (straddles midnight).
    let day1_hour = day_midnight - Duration::hours(1);
    let day2_hour = day_midnight;
    let day1_key = day1_hour.format("%Y-%m-%dT%H:00:00Z").to_string();

    storage
        .save_metrics(&sample_metrics(day1_hour + Duration::minutes(58), 10.0))
        .await
        .unwrap();
    storage
        .save_metrics(&sample_metrics(day2_hour + Duration::minutes(2), 90.0))
        .await
        .unwrap();

    storage.aggregate_hourly_metrics(day1_hour).await.unwrap();
    storage.aggregate_hourly_metrics(day2_hour).await.unwrap();

    let rows = storage.list_hourly_metrics_since(&day1_key).unwrap();
    assert_eq!(rows.len(), 2, "two distinct hour buckets expected");
    assert_eq!(rows[0].sample_count, 1);
    assert_eq!(rows[1].sample_count, 1);
    assert!(rows[1].hour.ends_with("T00:00:00Z"));
}

// ── process_snapshots ──────────────────────────────────────────
// save_process_snapshot + get_process_snapshots are covered by
// sqlite/tests.rs:225 `save_and_get_process_snapshot`. Only the
// cleanup-cutoff path is a residual gap (per Task 0 audit).

#[tokio::test]
async fn cleanup_old_process_snapshots_deletes_before_cutoff_only() {
    let storage = open_storage();
    let now = Utc::now();
    let old = now - Duration::days(45);
    let recent = now - Duration::minutes(5);

    storage
        .save_process_snapshot(&sample_process_snapshot(old, 1))
        .await
        .unwrap();
    storage
        .save_process_snapshot(&sample_process_snapshot(recent, 2))
        .await
        .unwrap();

    let cutoff = now - Duration::days(30);
    let deleted = storage.cleanup_old_process_snapshots(cutoff).await.unwrap();
    assert_eq!(deleted, 1, "1 row older than cutoff deleted");

    let remaining = storage
        .get_process_snapshots(now - Duration::days(100), now + Duration::minutes(1), 100)
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].processes.len(), 2, "recent snapshot preserved");
}

// ── idle_periods ───────────────────────────────────────────────
// start/end/get_ongoing(Some)/get_idle_periods covered by
// sqlite/tests.rs:241 `idle_period_lifecycle`. Residual gaps:
// fresh-DB None path + cleanup preserves active.

#[tokio::test]
async fn get_ongoing_idle_period_none_when_no_periods() {
    let storage = open_storage();
    let result = storage.get_ongoing_idle_period().await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn cleanup_old_idle_periods_preserves_active_even_if_start_is_old() {
    let storage = open_storage();
    let now = Utc::now();

    // An active period started 100 days ago — cleanup must NOT delete it.
    let _active_id = storage
        .start_idle_period(now - Duration::days(100))
        .await
        .unwrap();

    // An ended period from 50 days ago — cleanup SHOULD delete.
    let ended_id = storage
        .start_idle_period(now - Duration::days(50))
        .await
        .unwrap();
    storage
        .end_idle_period(ended_id, now - Duration::days(50) + Duration::seconds(60))
        .await
        .unwrap();

    let cutoff = now - Duration::days(30);
    let deleted = storage.cleanup_old_idle_periods(cutoff).await.unwrap();
    assert_eq!(deleted, 1, "only the ended old period deleted");

    let ongoing = storage.get_ongoing_idle_period().await.unwrap();
    assert!(ongoing.is_some(), "active period survived cleanup");
}

// ── sessions ───────────────────────────────────────────────────
// upsert/get/end + increment-on-existing covered by sqlite/tests.rs:267
// `session_stats_lifecycle`. Residual gap: increment on nonexistent.

#[tokio::test]
async fn increment_session_counters_on_nonexistent_is_noop() {
    let storage = open_storage();
    // No upsert first — just call increment on a session_id that doesn't exist.
    let result = storage
        .increment_session_counters("does-not-exist", 1, 1, 1)
        .await;
    // SQLite UPDATE on 0 rows is not an error.
    assert!(result.is_ok());

    let got = storage.get_session("does-not-exist").await.unwrap();
    assert!(got.is_none(), "increment must not create the row");
}

// ── sync list helpers ──────────────────────────────────────────

#[tokio::test]
async fn list_session_stats_orders_by_started_at_desc_and_respects_limit() {
    let storage = open_storage();

    // Upsert 3 sessions with different started_at timestamps.
    for (i, started_delta_sec) in [100_i64, 200, 300].iter().enumerate() {
        let mut s = sample_session(&format!("sess-{i}"), 0, 0);
        s.started_at = Utc::now() - Duration::seconds(*started_delta_sec);
        storage.upsert_session(&s).await.unwrap();
    }

    // limit = 2 returns the 2 most recent (smallest delta — sess-0 and sess-1).
    let results = storage.list_session_stats(2).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].session_id, "sess-0");
    assert_eq!(results[1].session_id, "sess-1");
}

#[tokio::test]
async fn list_session_stats_empty_db_returns_empty_vec() {
    let storage = open_storage();
    let results = storage.list_session_stats(10).unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn list_hourly_metrics_since_filters_by_from_hour() {
    let storage = open_storage();
    // Anchor to the hour AT LEAST 3 hours in the past.
    let anchor = current_hour_start() - Duration::hours(3);

    // Seed 3 hours of raw samples, then aggregate each.
    for h in 0..3 {
        let hour_start = anchor + Duration::hours(h);
        storage
            .save_metrics(&sample_metrics(hour_start + Duration::minutes(30), 25.0))
            .await
            .unwrap();
        storage.aggregate_hourly_metrics(hour_start).await.unwrap();
    }

    // Filter: from hour+1 onward — should see 2 rows (h+1 and h+2).
    let from_key = (anchor + Duration::hours(1))
        .format("%Y-%m-%dT%H:00:00Z")
        .to_string();
    let results = storage.list_hourly_metrics_since(&from_key).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].hour, from_key);
}

// ── lock-contract regression ───────────────────────────────────
// NOT race tests — Arc<Mutex<Connection>> already serializes. These
// tests assert the Mutex contract survives across multiple async tasks
// on a multi-thread tokio runtime. No overlap with
// sqlite/tests.rs:139 `concurrent_save_and_get` (that tests save_event,
// not save_metrics / save_process_snapshot).

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_save_metrics_preserves_count() {
    let storage = Arc::new(open_storage());
    let base = Utc::now();

    // Spawn 4 concurrent writer tasks, each inserting 5 samples.
    let mut handles = Vec::new();
    for task_id in 0..4_i64 {
        let s = storage.clone();
        let task_base = base + Duration::seconds(task_id * 10);
        handles.push(tokio::spawn(async move {
            for i in 0..5_i64 {
                let ts = task_base + Duration::milliseconds(i);
                s.save_metrics(&sample_metrics(ts, 10.0 + task_id as f32))
                    .await
                    .unwrap();
            }
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // Total = 4 × 5 = 20 rows.
    let results = storage
        .get_metrics(
            base - Duration::minutes(1),
            base + Duration::minutes(10),
            1_000,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 20);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_writer_plus_reader_yield_consistent_snapshot() {
    let storage = Arc::new(open_storage());
    let ts_start = Utc::now();

    let writer = {
        let s = storage.clone();
        tokio::spawn(async move {
            for i in 0..50_i64 {
                s.save_metrics(&sample_metrics(ts_start + Duration::milliseconds(i), 20.0))
                    .await
                    .unwrap();
            }
        })
    };

    let reader = {
        let s = storage.clone();
        tokio::spawn(async move {
            let mut max_observed = 0usize;
            for _ in 0..10 {
                let results = s
                    .get_metrics(
                        ts_start - Duration::seconds(1),
                        ts_start + Duration::seconds(10),
                        1_000,
                    )
                    .await
                    .unwrap();
                // Invariant: count monotonic via Arc<Mutex> serialization;
                // never observe more than the writer has committed.
                assert!(results.len() <= 50);
                max_observed = max_observed.max(results.len());
                // Yield so writer gets a chance to make progress.
                tokio::task::yield_now().await;
            }
            // Without this the test trivially passes if the reader completes
            // before the writer runs (observing 0 each iteration). Requiring
            // a non-zero observation somewhere forces the reader to witness
            // the writer's effect.
            assert!(
                max_observed > 0,
                "reader never observed writer's effect — test may be racing trivially"
            );
        })
    };

    writer.await.unwrap();
    reader.await.unwrap();

    let final_count = storage
        .get_metrics(
            ts_start - Duration::seconds(1),
            ts_start + Duration::seconds(10),
            1_000,
        )
        .await
        .unwrap()
        .len();
    assert_eq!(final_count, 50);
}

// ── Err branches + unreachable-Err documentation ───────────────
//
// TODO: untested Err — save_metrics StorageError::Internal on SQLite
// failure. The Connection is held inside SqliteStorage for its lifetime;
// there is no path to force a closed-connection error without unsafe code.
// Equivalent coverage would require injecting a mock Connection, which
// is out of scope for D8. If this ever needs coverage, extract a
// `with_conn` trait with a failure-injecting test double.
//
// TODO: untested Err — get_metrics RFC3339 parse fallback.
// The fallback branch (`unwrap_or_else(|_| Utc::now())`) IS reachable
// in production if a row has a malformed timestamp column, but cannot
// be deterministically exercised from a test because SQL string-ordering
// on 'not-a-date' vs RFC3339 range bounds is undefined for our purposes.
// Also it is a SILENT FALLBACK (returns a value, not Err), so even a
// successful trigger would not exercise an Err branch.

#[tokio::test]
async fn get_process_snapshots_invalid_json_in_column_silently_defaults_to_empty() {
    let storage = open_storage();
    let arc = storage.connection_arc();

    // Direct-insert a row with malformed JSON in snapshot_data.
    {
        let conn = arc.lock().unwrap();
        conn.execute(
            "INSERT INTO process_snapshots (timestamp, snapshot_data) VALUES (?1, ?2)",
            rusqlite::params![Utc::now().to_rfc3339(), "{not:valid json"],
        )
        .unwrap();
    }

    // get_process_snapshots uses `serde_json::from_str(&data).unwrap_or_default()`
    // so the bad row is returned with empty processes — not an Err.
    let results = storage
        .get_process_snapshots(
            Utc::now() - Duration::minutes(1),
            Utc::now() + Duration::minutes(1),
            10,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0].processes.is_empty(),
        "malformed JSON silently becomes empty processes — this is by design per \
         the unwrap_or_default() call in the module"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn save_metrics_mutex_poison_returns_err_variant_only() {
    let storage = open_storage();
    let conn_arc = storage.connection_arc();

    // Poison the lock from a blocking task.
    let c = conn_arc.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let _guard = c.lock().unwrap();
        panic!("intentional poison");
    })
    .await;

    // The subsequent save_metrics should propagate as CoreError::Storage.
    // Variant-only match — error string varies per call site (see spec Section 7).
    // iter-47: changed from Internal → Storage when StorageError::Internal
    // mapping was redirected to storage.failed (observed failure IS a storage op).
    let result = storage
        .save_metrics(&sample_metrics(Utc::now(), 10.0))
        .await;
    assert!(
        matches!(result, Err(CoreError::Storage { .. })),
        "expected Err(CoreError::Storage {{ .. }}), got {result:?}"
    );
}

// ── contract-covered edge cases ────────────────────────────────

#[tokio::test]
async fn save_metrics_null_network_roundtrips_as_zeros() {
    let storage = open_storage();
    let ts = Utc::now();
    storage
        .save_metrics(&sample_metrics_no_network(ts, 15.0))
        .await
        .unwrap();

    let results = storage
        .get_metrics(ts - Duration::seconds(1), ts + Duration::seconds(1), 10)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    // save_metrics writes (0, 0) for null network; get_metrics reconstructs
    // a NetworkInfo { upload=0, download=0, is_connected=false }.
    let got = &results[0];
    assert!(got.network.is_some());
    let net = got.network.as_ref().unwrap();
    assert_eq!(net.upload_speed, 0);
    assert_eq!(net.download_speed, 0);
    assert!(!net.is_connected);
}

#[tokio::test]
async fn save_and_get_metrics_bulk_100_samples_round_trips_all() {
    let storage = open_storage();
    let base = Utc::now();

    for i in 0..100_i64 {
        storage
            .save_metrics(&sample_metrics(base + Duration::milliseconds(i), i as f32))
            .await
            .unwrap();
    }

    let results = storage
        .get_metrics(
            base - Duration::seconds(1),
            base + Duration::seconds(10),
            200,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 100);
}

#[tokio::test]
async fn cleanup_old_metrics_deletes_before_cutoff_preserves_after() {
    let storage = open_storage();
    let now = Utc::now();

    // 3 old, 2 recent.
    for i in 0..3_i64 {
        storage
            .save_metrics(&sample_metrics(
                now - Duration::days(45) + Duration::seconds(i),
                10.0,
            ))
            .await
            .unwrap();
    }
    for i in 0..2_i64 {
        storage
            .save_metrics(&sample_metrics(
                now - Duration::minutes(5) + Duration::seconds(i),
                20.0,
            ))
            .await
            .unwrap();
    }

    let cutoff = now - Duration::days(30);
    let deleted = storage.cleanup_old_metrics(cutoff).await.unwrap();
    assert_eq!(deleted, 3, "3 old rows deleted");

    let remaining = storage
        .get_metrics(now - Duration::days(100), now + Duration::days(1), 100)
        .await
        .unwrap();
    assert_eq!(remaining.len(), 2);
}
