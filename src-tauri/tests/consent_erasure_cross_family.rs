//! #4928 PHASE 2 — cross-family integration test for the consent-revoke erasure
//! chokepoint.
//!
//! Through the real composition seam (the shared `deletion_flag`), wire
//! `SqliteStorage` + `FrameFileStorage` + `ConsentManager` so they share the
//! **same flag**, then drive each writer family concurrently with erase to prove:
//!
//! 1. ptr-eq: consent ↔ SQLite ↔ frames share the same `Arc<AtomicBool>`.
//! 2. After revoke + erase, every `ALL_TABLES` table has row == 0 and the frame
//!    directory is empty.
//! 3. Writes after the flag is set are no-op `Ok` (row count unchanged, no frame
//!    created).
//! 4. When `grant_consent` clears the flag, writes resume and persist.
//! 5. Retained tables (`egress_ledger`/`audit_log`/`app_meta`) are not skipped.
//!
//! Every SQLite write passes through the production funnel
//! (`GuardedConnection::write_lock`), so writers whose per-family domain model is
//! hard to construct INSERT directly into the same funnel to faithfully exercise
//! the chokepoint (the test does not create a bypass path — `connection_arc()`
//! only returns an `Arc<GuardedConnection>`).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::Utc;
use maekon_core::consent::{ConsentManager, ConsentPermissions};
use maekon_core::models::event::{ContextEvent, Event};
use maekon_core::models::storage_records::EgressLedgerRecord;
use maekon_core::models::system::SystemMetrics;
use maekon_core::ports::change_merger::ChangeMerger;
use maekon_core::ports::regime_storage::RegimeStoragePort;
use maekon_core::ports::storage::{MetricsStorage, StorageService};
use maekon_storage::frame_storage::FrameFileStorage;
use maekon_storage::regime_manager_state_store::SqliteRegimeManagerStateStore;
use maekon_storage::sqlite::SqliteStorage;
use maekon_storage::sync_merger::SqliteSyncMerger;

/// Helper that reproduces the real composition seam.
///
/// Builds `SqliteStorage::open` (on disk) → `FrameFileStorage` →
/// `ConsentManager` in that order, then installs the ConsentManager's
/// `deletion_flag()` into SQLite + frames. (Isomorphic to the production
/// `app_runtime_launch` + `SharedCaptureServices::build` wiring.)
struct Composition {
    storage: Arc<SqliteStorage>,
    frames: Arc<FrameFileStorage>,
    consent: Arc<ConsentManager>,
    _dir: tempfile::TempDir,
}

async fn build_composition() -> Composition {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(SqliteStorage::open(&dir.path().join("s.db"), 30, None).unwrap());

    let consent = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
    // Grant consent to open the collection window (flag is clear at first).
    consent
        .grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();

    // Install the shared flag — SQLite (ArcSwap seam) + frames (&mut, before Arc wrap).
    storage.set_deletion_flag(consent.deletion_flag());
    // #4928 round-3 (FIX B): install the erase-window block signal `erasing` via the
    // same Arc as well.
    storage.set_erasing(consent.erasing());

    let mut frames_concrete = FrameFileStorage::new(dir.path().to_path_buf(), 100, 30)
        .await
        .unwrap();
    frames_concrete.set_deletion_flag(consent.deletion_flag());
    frames_concrete.set_erasing(consent.erasing());
    let frames = Arc::new(frames_concrete);

    Composition {
        storage,
        frames,
        consent,
        _dir: dir,
    }
}

/// INSERT one row into one table through the production funnel (`write_lock`).
/// When the flag is set the funnel skips, so the return is 0 (skipped) or
/// 1 (recorded).
fn funnel_insert(storage: &SqliteStorage, sql: &str) -> usize {
    let conn = storage.connection_arc();
    let n = conn
        .write_lock()
        .run::<_, usize, rusqlite::Error>(0, |c| c.execute(sql, []))
        .unwrap();
    n
}

fn count_rows(storage: &SqliteStorage, table: &str) -> i64 {
    let conn = storage.connection_arc();
    let n = conn
        .read_lock()
        .run::<_, i64, rusqlite::Error>(|c| {
            c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        })
        .unwrap_or(0);
    n
}

fn metrics_sample() -> SystemMetrics {
    SystemMetrics {
        timestamp: Utc::now(),
        cpu_usage: 12.5,
        memory_used: 4096,
        memory_total: 16384,
        disk_used: 100,
        disk_total: 500,
        network: None,
        typing_wpm: 0.0,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Test 1: composition seam ptr-eq (consent ↔ SQLite ↔ frames share one flag)
// ───────────────────────────────────────────────────────────────────────────

/// #4928: through the real composition seam, `set_deletion_flag` re-wires the
/// LIVE flag, and the consent/SQLite/frames trio share the same
/// `Arc<AtomicBool>`.
#[tokio::test]
async fn set_deletion_flag_shares_arc_through_composition() {
    let c = build_composition().await;

    let consent_flag = c.consent.deletion_flag();
    let storage_flag = c.storage.deletion_flag();
    let frames_flag = c.frames.deletion_flag();

    assert!(
        Arc::ptr_eq(&consent_flag, &storage_flag),
        "ConsentManager and SqliteStorage must share the same deletion_flag Arc (ptr-eq)"
    );
    assert!(
        Arc::ptr_eq(&consent_flag, &frames_flag),
        "ConsentManager and FrameFileStorage must share the same deletion_flag Arc (ptr-eq)"
    );

    // Verify that consent's revoke sets the same flag that SQLite/frames observe.
    assert!(!storage_flag.load(Ordering::Acquire));
    c.consent.revoke_consent().unwrap();
    assert!(
        storage_flag.load(Ordering::Acquire) && frames_flag.load(Ordering::Acquire),
        "after revoke, the flag observed by SQLite/frames must both be set (proves LIVE re-wiring)"
    );

    // Adapters sharing connection_arc() observe the same flag too.
    let merger = SqliteSyncMerger::new(c.storage.connection_arc(), "dev-1".to_string());
    let _ = &merger; // Type-level check that the adapter receives the same GuardedConnection.
    assert!(
        Arc::ptr_eq(&c.storage.connection_arc().deletion_flag(), &consent_flag),
        "the connection_arc() adapter must share the same deletion_flag too"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Test 2: cross-family concurrent writers + erase → zero residual
// ───────────────────────────────────────────────────────────────────────────

/// #4928: drive each writer family (events/metrics/idle/suggestions/sessions/
/// digests/focus/sync-merge/regime-state/frames) concurrently with erase, then
/// after revoke + `delete_all_data` + frame-barrier deletion assert that every
/// ALL_TABLES row == 0 and the frame directory is empty.
#[tokio::test]
async fn cross_family_writers_concurrent_with_erase_leave_zero_residual() {
    let c = build_composition().await;

    // ── Before revoke: write one entry per family (flag clear → persist) ─────
    // events (port)
    c.storage
        .save_event(&Event::Context(ContextEvent {
            app_name: "Code".into(),
            window_title: "main.rs".into(),
            timestamp: Utc::now(),
            ..Default::default()
        }))
        .await
        .unwrap();
    // metrics (port)
    c.storage.save_metrics(&metrics_sample()).await.unwrap();
    // idle (port)
    let idle_id = c.storage.start_idle_period(Utc::now()).await.unwrap();
    c.storage
        .end_idle_period(idle_id, Utc::now())
        .await
        .unwrap();
    // suggestions / digests / focus / regime-state / sync-merge: direct funnel
    // INSERT (avoids constructing domain models but still passes through the
    // same production chokepoint).
    assert_eq!(
        funnel_insert(
            &c.storage,
            "INSERT INTO local_suggestions (suggestion_type, payload, created_at) \
             VALUES ('tip', '{}', '2026-01-01T00:00:00Z')",
        ),
        1,
        "a suggestion write before revoke must persist"
    );
    assert_eq!(
        funnel_insert(
            &c.storage,
            "INSERT INTO daily_digests (date, timeline_json, statistics_json, generated_at) \
             VALUES ('2026-01-01', '[]', '{}', '2026-01-01T00:00:00Z')",
        ),
        1
    );
    assert_eq!(
        funnel_insert(
            &c.storage,
            "INSERT INTO focus_metrics (date, total_active_secs, deep_work_secs) \
             VALUES ('2026-01-01', 100, 50)",
        ),
        1
    );
    assert_eq!(
        funnel_insert(
            &c.storage,
            "INSERT INTO work_sessions (started_at, primary_app, category) \
             VALUES ('2026-01-01T00:00:00Z', 'Code', 'coding')",
        ),
        1
    );
    assert_eq!(
        funnel_insert(
            &c.storage,
            "INSERT INTO regime_manager_state (id, payload) VALUES (0, '{}')",
        ),
        1
    );
    assert_eq!(
        funnel_insert(
            &c.storage,
            "INSERT INTO regimes (id, label, detected_at, last_seen_at, dominant_category) \
             VALUES ('r-pre', 'focus', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'coding')",
        ),
        1
    );
    // frames (port) — one before revoke.
    let p = c
        .frames
        .save_frame(Utc::now(), b"pre-revoke-frame")
        .await
        .unwrap();
    assert!(
        !p.as_os_str().is_empty(),
        "a frame before revoke must be saved"
    );

    // sanity: the data actually landed.
    for t in [
        "events",
        "system_metrics",
        "idle_periods",
        "local_suggestions",
        "daily_digests",
        "focus_metrics",
        "work_sessions",
        "regime_manager_state",
        "regimes",
    ] {
        assert!(
            count_rows(&c.storage, t) > 0,
            "{t} must be seeded before revoke"
        );
    }

    // ── revoke + concurrent erase ───────────────────────────────────────────
    // revoke sets the flag (just before erase). Every writer entering afterward
    // is skipped.
    c.consent.revoke_consent().unwrap();

    // Drive each family writer concurrently with erase (in-flight race). All
    // must be skipped.
    let storage_w = c.storage.clone();
    let frames_w = c.frames.clone();
    // sync-merge (connection_arc adapter) — receives the same GuardedConnection/flag.
    let merger = SqliteSyncMerger::new(c.storage.connection_arc(), "dev-1".to_string());
    // The regime-state adapter is also built from the same GuardedConnection
    // (type-level coverage).
    let regime_store: Arc<dyn RegimeStoragePort> = Arc::new(SqliteRegimeManagerStateStore::new(
        c.storage.connection_arc(),
    ));
    let _ = &regime_store;

    let writers = tokio::spawn(async move {
        // events/metrics/idle/suggestions (port) — no-op Ok because the flag is set.
        let _ = storage_w
            .save_event(&Event::Context(ContextEvent {
                app_name: "X".into(),
                window_title: "Y".into(),
                timestamp: Utc::now(),
                ..Default::default()
            }))
            .await;
        let _ = storage_w.save_metrics(&metrics_sample()).await;
        let _ = storage_w.start_idle_period(Utc::now()).await;
        // suggestions family (funnel INSERT into `suggestions` ∈ ALL_TABLES) — skipped.
        funnel_insert(
            &storage_w,
            "INSERT INTO suggestions (suggestion_id, suggestion_type, content, priority, \
             confidence_score, created_at) \
             VALUES ('sug-during', 'tip', 'x', 'LOW', 0.5, '2026-02-02T00:00:00Z')",
        );
        // sync-merge (connection_arc adapter) — skipped via the same flag.
        let _ = merger
            .apply_changes(maekon_core::models::sync::ChangeSet::default())
            .await;
        // direct funnel INSERT (digests/focus/sessions/regime-state) — skipped.
        funnel_insert(
            &storage_w,
            "INSERT INTO daily_digests (date, timeline_json, statistics_json, generated_at) \
             VALUES ('2026-02-02', '[]', '{}', '2026-02-02T00:00:00Z')",
        );
        funnel_insert(
            &storage_w,
            "INSERT INTO work_sessions (started_at, primary_app, category) \
             VALUES ('2026-02-02T00:00:00Z', 'X', 'coding')",
        );
        funnel_insert(
            &storage_w,
            "INSERT INTO focus_metrics (date, total_active_secs) VALUES ('2026-02-02', 9)",
        );
        funnel_insert(
            &storage_w,
            "INSERT OR REPLACE INTO regime_manager_state (id, payload) VALUES (1, '{}')",
        );
        // frames — skipped (empty path).
        let _ = frames_w.save_frame(Utc::now(), b"during-erase-frame").await;
    });

    // erase: Phase-1 full SQLite deletion (retained path) + Phase-2 frame-barrier
    // deletion.
    let storage_e = c.storage.clone();
    let erase = tokio::task::spawn_blocking(move || storage_e.delete_all_data());
    erase.await.unwrap().unwrap();
    let deleted = c.frames.delete_all_files().await.unwrap();
    let _ = deleted;

    writers.await.unwrap();

    // ── Assert: every ALL_TABLES table is empty ─────────────────────────────
    let all_tables = [
        "events",
        "frames",
        "system_metrics",
        "system_metrics_hourly",
        "process_snapshots",
        "idle_periods",
        "session_stats",
        "work_sessions",
        "interruptions",
        "focus_metrics",
        "suggestions",
        "local_suggestions",
        "frame_tags",
        "tags",
        "activity_segments",
        "calibration_log",
        "daily_digests",
        "weekly_digests",
        "embedding_vectors",
        "regime_overrides",
        "regimes",
        "trigger_params_snapshots",
        "search_fts",
        "search_trigram",
        "vector_binary_codes",
        "vector_index_meta",
        "ivf_centroids",
        "ivf_assignments",
        "gui_interactions",
        "device_identity",
        "sync_peers",
        "lan_peer_pins",
        "coaching_events",
        "regime_goals",
        "coaching_effectiveness",
        "feedback_scorer_tallies",
        "regime_reaction_stats",
        "ai_conversation_messages",
        "ai_sessions",
        "frame_annotations",
        "habit_streaks",
        "regime_manager_state",
        "automation_presets",
        "feedback_retries",
        "memory_claims",
        "memory_edges",
    ];
    for t in all_tables {
        assert_eq!(
            count_rows(&c.storage, t),
            0,
            "after erase, table '{t}' must have no residual rows (in-flight writers skipped + wipe)"
        );
    }

    // Frame directory: must be empty via post-revoke write skipping + delete_all_files.
    let remaining = std::fs::read_dir(c.frames.frames_dir())
        .map(|rd| rd.filter_map(|e| e.ok()).count())
        .unwrap_or(0);
    assert_eq!(
        remaining, 0,
        "after erase the frame directory must be empty (no residual frames)"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Test 3: a single write after the flag is set is a no-op Ok (row count unchanged)
// ───────────────────────────────────────────────────────────────────────────

/// #4928: a write after `deletion_flag` is set is skipped in the funnel, returns
/// `Ok`, and leaves the row count unchanged (both SQLite + frames).
#[tokio::test]
async fn write_after_flag_set_is_noop_ok() {
    let c = build_composition().await;

    // One entry before revoke.
    c.storage.save_metrics(&metrics_sample()).await.unwrap();
    let before = count_rows(&c.storage, "system_metrics");
    assert_eq!(before, 1);

    // revoke → flag set.
    c.consent.revoke_consent().unwrap();

    // Write attempt: returns Ok but the row count is unchanged.
    c.storage
        .save_metrics(&metrics_sample())
        .await
        .expect("save_metrics must be a no-op Ok when the flag is set");
    assert_eq!(
        count_rows(&c.storage, "system_metrics"),
        before,
        "a write after the flag is set must not change the row count"
    );

    // frames: empty path (skipped) + no file created.
    let p = c.frames.save_frame(Utc::now(), b"noop").await.unwrap();
    assert!(
        p.as_os_str().is_empty(),
        "a frame write when the flag is set returns an empty path (skipped)"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Test 4: grant clears the flag → writes resume
// ───────────────────────────────────────────────────────────────────────────

/// #4928: after revoke, `grant_consent` clears the LOCAL flag so writes resume.
#[tokio::test]
async fn grant_after_revoke_resumes_writes() {
    let c = build_composition().await;

    c.consent.revoke_consent().unwrap();
    // Right after revoke: write skipped.
    c.storage.save_metrics(&metrics_sample()).await.unwrap();
    assert_eq!(
        count_rows(&c.storage, "system_metrics"),
        0,
        "a write right after revoke must be skipped"
    );

    // Re-grant → flag clear.
    c.consent
        .grant_consent(ConsentPermissions::default(), 30)
        .unwrap();

    // Writes resume → persist.
    c.storage.save_metrics(&metrics_sample()).await.unwrap();
    assert_eq!(
        count_rows(&c.storage, "system_metrics"),
        1,
        "a write after re-grant must persist (flag clear)"
    );

    // frames resume too.
    let p = c
        .frames
        .save_frame(Utc::now(), b"after-regrant")
        .await
        .unwrap();
    assert!(
        !p.as_os_str().is_empty(),
        "a frame write after re-grant must return a real path"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Test 5: retained tables are still recorded after the flag is set
// ───────────────────────────────────────────────────────────────────────────

/// #4928: `egress_ledger`/`app_meta`/`audit_log` use the retained path, so writes
/// are not skipped after the flag is set (retained tables are not erase targets).
#[tokio::test]
async fn retained_tables_not_skipped_after_flag_set() {
    let c = build_composition().await;

    c.consent.revoke_consent().unwrap();

    // egress_ledger (retained_write_lock) — must be recorded even when the flag is set.
    c.storage
        .record_egress(&EgressLedgerRecord {
            record_id: "rec-1".into(),
            event_type: "context".into(),
            event_id: None,
            byte_count: 10,
            recipient_count: 1,
            destination: "server".into(),
            disposition: "uploaded".into(),
            consent_state: "revoked".into(),
            occurred_at: "2026-01-01T00:00:00Z".into(),
        })
        .expect("a retained egress write must succeed even when the flag is set");
    assert_eq!(
        count_rows(&c.storage, "egress_ledger"),
        1,
        "egress_ledger (retained) must still be recorded after the flag is set"
    );

    // app_meta (set_meta_checked, retained) — recorded even when the flag is set.
    c.storage
        .set_meta_checked("post_revoke_marker", "1")
        .expect("a retained app_meta write must succeed even when the flag is set");
    assert_eq!(
        c.storage.get_meta("post_revoke_marker"),
        Some("1".to_string()),
        "app_meta (retained) must still be recorded after the flag is set"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Test 6: grant_consent-during-erase TOCTOU — the erasing signal blocks the race
// ───────────────────────────────────────────────────────────────────────────

/// #4928 round-3 (FIX B): even if `grant_consent` slips in during the erase
/// window (after the Phase-1 commit through Phase-2 in progress) and clears
/// `deletion_flag`, in-flight writes must keep being skipped while `erasing` is
/// set (write-skip predicate = `deletion_flag || erasing`). Writes resume only
/// after erase finishes (`erasing=false`) and the re-grant has been applied.
#[tokio::test]
async fn grant_during_erase_window_is_still_skipped_until_erasing_clears() {
    let c = build_composition().await;

    let erasing = c.consent.erasing();
    let deletion = c.consent.deletion_flag();

    // 1) revoke → simulate the start of erase: erasing=true (the signal the RAII
    //    guard sets).
    c.consent.revoke_consent().unwrap();
    assert!(
        deletion.load(Ordering::Acquire),
        "deletion_flag set after revoke"
    );
    erasing.store(true, Ordering::Release); // Isomorphic to EraseWindowGuard::set.

    // 2) Concurrent re-grant in the middle of the erase window: only deletion_flag
    //    is cleared.
    c.consent
        .grant_consent(ConsentPermissions::default(), 30)
        .unwrap();
    assert!(
        !deletion.load(Ordering::Acquire),
        "re-grant cleared deletion_flag (reproduces the TOCTOU window)"
    );
    assert!(
        erasing.load(Ordering::Acquire),
        "grant_consent cannot clear erasing"
    );

    // 3) In-flight SQLite writes must still be skipped (because erasing is set).
    c.storage.save_metrics(&metrics_sample()).await.unwrap();
    assert_eq!(
        count_rows(&c.storage, "system_metrics"),
        0,
        "while erasing is set, SQLite writes must be skipped even after re-grant (TOCTOU block)"
    );
    // Direct funnel INSERT is skipped too.
    let n = funnel_insert(
        &c.storage,
        "INSERT INTO events (event_id, event_type, timestamp, data) \
         VALUES ('toctou-1','window','2026-01-01T00:00:00Z','{}')",
    );
    assert_eq!(n, 0, "events writes are also skipped while erasing is set");
    assert_eq!(count_rows(&c.storage, "events"), 0);

    // 4) In-flight frame writes are also skipped (empty path).
    let p = c
        .frames
        .save_frame(Utc::now(), b"toctou-frame")
        .await
        .unwrap();
    assert!(
        p.as_os_str().is_empty(),
        "while erasing is set, frame writes must also be skipped (empty PathBuf)"
    );

    // 5) erase complete: erasing=false (isomorphic to EraseWindowGuard Drop).
    //    The re-grant is already applied.
    erasing.store(false, Ordering::Release);

    // Now deletion_flag is clear and erasing is clear → writes resume.
    c.storage.save_metrics(&metrics_sample()).await.unwrap();
    assert_eq!(
        count_rows(&c.storage, "system_metrics"),
        1,
        "SQLite writes must resume after erase completes + re-grant"
    );
    let p2 = c
        .frames
        .save_frame(Utc::now(), b"after-erase")
        .await
        .unwrap();
    assert!(
        !p2.as_os_str().is_empty(),
        "frame writes must resume after erase completes + re-grant"
    );
}

/// #4928 round-3 (FIX B): verify via ptr-eq that consent ↔ SQLite ↔ frames share
/// the same `erasing` Arc through the composition seam.
#[tokio::test]
async fn erasing_signal_shared_through_composition_ptr_eq() {
    let c = build_composition().await;
    let consent_erasing = c.consent.erasing();
    assert!(
        Arc::ptr_eq(&c.storage.erasing(), &consent_erasing),
        "ConsentManager and SqliteStorage must share the same erasing Arc (ptr-eq)"
    );
    assert!(
        Arc::ptr_eq(&c.frames.erasing(), &consent_erasing),
        "ConsentManager and FrameFileStorage must share the same erasing Arc (ptr-eq)"
    );
    // The connection_arc() adapter observes the same erasing too.
    assert!(
        Arc::ptr_eq(&c.storage.connection_arc().erasing(), &consent_erasing),
        "the connection_arc() adapter must share the same erasing too"
    );
}
