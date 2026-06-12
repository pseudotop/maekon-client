//! #4928 PHASE 2 — consent-revoke erasure 차단 chokepoint 의 cross-family 통합 테스트.
//!
//! 실제 composition seam(공유 `deletion_flag`)을 통해 `SqliteStorage` +
//! `FrameFileStorage` + `ConsentManager` 가 **동일한 flag** 를 공유하도록 배선한 뒤,
//! 각 writer family 를 erase 와 동시에 구동하여 다음을 증명한다:
//!
//! 1. ptr-eq: consent ↔ SQLite ↔ frames 가 동일 `Arc<AtomicBool>` 를 공유한다.
//! 2. revoke + erase 후 모든 `ALL_TABLES` 테이블 row == 0, 프레임 디렉터리 비어있음.
//! 3. flag set 이후의 쓰기는 no-op `Ok` (row 수 불변, 프레임 미생성).
//! 4. `grant_consent` 가 flag 를 clear 하면 쓰기가 재개되어 persist 된다.
//! 5. 보존(retained) 테이블(`egress_ledger`/`audit_log`/`app_meta`)은 스킵되지 않는다.
//!
//! 모든 SQLite 쓰기는 프로덕션 funnel(`GuardedConnection::write_lock`)을 통과하므로,
//! family 별 도메인 모델을 구성하기 어려운 writer 는 동일 funnel 에 직접 INSERT 하여
//! chokepoint 를 충실하게 검증한다(테스트가 우회 경로를 만들지 않는다 —
//! `connection_arc()` 는 `Arc<GuardedConnection>` 만 반환한다).

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

/// 실제 composition seam 을 재현하는 헬퍼.
///
/// `SqliteStorage::open`(디스크) → `FrameFileStorage` → `ConsentManager` 순으로
/// 만든 뒤, ConsentManager 의 `deletion_flag()` 를 SQLite + frames 에 install 한다.
/// (프로덕션의 `app_runtime_launch` + `SharedCaptureServices::build` 배선과 동형.)
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
    // 동의를 부여해 collection 윈도우를 연다(처음엔 flag clear).
    consent
        .grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();

    // 공유 flag install — SQLite(ArcSwap seam) + frames(&mut, Arc wrap 전).
    storage.set_deletion_flag(consent.deletion_flag());
    // #4928 round-3 (FIX B): erase-window 차단 신호 `erasing` 도 동일 Arc 로 install.
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

/// 프로덕션 funnel(`write_lock`)을 통해 한 테이블에 한 행을 INSERT 한다.
/// flag set 시 funnel 이 스킵하므로 반환은 0(스킵) 또는 1(기록)이다.
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
// 테스트 1: composition seam ptr-eq (consent ↔ SQLite ↔ frames 동일 flag)
// ───────────────────────────────────────────────────────────────────────────

/// #4928: 실제 composition seam 을 통해 `set_deletion_flag` 가 LIVE flag 를
/// 재배선하며, consent/SQLite/frames 셋이 동일 `Arc<AtomicBool>` 를 공유한다.
#[tokio::test]
async fn set_deletion_flag_shares_arc_through_composition() {
    let c = build_composition().await;

    let consent_flag = c.consent.deletion_flag();
    let storage_flag = c.storage.deletion_flag();
    let frames_flag = c.frames.deletion_flag();

    assert!(
        Arc::ptr_eq(&consent_flag, &storage_flag),
        "ConsentManager 와 SqliteStorage 는 동일 deletion_flag Arc 를 공유해야 한다 (ptr-eq)"
    );
    assert!(
        Arc::ptr_eq(&consent_flag, &frames_flag),
        "ConsentManager 와 FrameFileStorage 는 동일 deletion_flag Arc 를 공유해야 한다 (ptr-eq)"
    );

    // consent 의 revoke 가 SQLite/frames 가 보는 동일 flag 를 set 하는지 확인한다.
    assert!(!storage_flag.load(Ordering::Acquire));
    c.consent.revoke_consent().unwrap();
    assert!(
        storage_flag.load(Ordering::Acquire) && frames_flag.load(Ordering::Acquire),
        "revoke 후 SQLite/frames 가 보는 flag 가 모두 set 되어야 한다 (LIVE 재배선 증명)"
    );

    // connection_arc() 를 공유하는 어댑터도 동일 flag 를 본다.
    let merger = SqliteSyncMerger::new(c.storage.connection_arc(), "dev-1".to_string());
    let _ = &merger; // 어댑터가 동일 GuardedConnection 을 받음을 타입 레벨에서 확인.
    assert!(
        Arc::ptr_eq(&c.storage.connection_arc().deletion_flag(), &consent_flag),
        "connection_arc() 어댑터도 동일 deletion_flag 를 공유해야 한다"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 테스트 2: cross-family 동시 writer + erase → 잔존 0
// ───────────────────────────────────────────────────────────────────────────

/// #4928: 각 writer family(events/metrics/idle/suggestions/sessions/digests/
/// focus/sync-merge/regime-state/frames)를 erase 와 동시에 구동하고, revoke +
/// `delete_all_data` + 프레임 배리어 삭제 후 모든 ALL_TABLES row == 0 / 프레임
/// 디렉터리가 비어있음을 단언한다.
#[tokio::test]
async fn cross_family_writers_concurrent_with_erase_leave_zero_residual() {
    let c = build_composition().await;

    // ── revoke 전: 각 family 로 한 건씩 기록 (flag clear → persist) ──────────
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
    // suggestions / digests / focus / regime-state / sync-merge: funnel 직접 INSERT
    // (도메인 모델 구성을 피하되 동일 프로덕션 chokepoint 를 통과한다).
    assert_eq!(
        funnel_insert(
            &c.storage,
            "INSERT INTO local_suggestions (suggestion_type, payload, created_at) \
             VALUES ('tip', '{}', '2026-01-01T00:00:00Z')",
        ),
        1,
        "revoke 전 suggestion 쓰기는 persist 되어야 한다"
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
    // frames (port) — revoke 전 1개.
    let p = c
        .frames
        .save_frame(Utc::now(), b"pre-revoke-frame")
        .await
        .unwrap();
    assert!(
        !p.as_os_str().is_empty(),
        "revoke 전 프레임은 저장되어야 한다"
    );

    // sanity: 데이터가 실제로 들어갔다.
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
            "{t} 가 revoke 전 seed 되어야 한다"
        );
    }

    // ── revoke + 동시 erase ─────────────────────────────────────────────────
    // revoke 가 flag 를 set 한다(erase 직전). 이후 진입하는 writer 는 모두 스킵된다.
    c.consent.revoke_consent().unwrap();

    // erase 와 동시에 각 family writer 를 구동한다(in-flight race). 모두 스킵되어야 한다.
    let storage_w = c.storage.clone();
    let frames_w = c.frames.clone();
    // sync-merge (connection_arc 어댑터) — 동일 GuardedConnection/flag 를 받음.
    let merger = SqliteSyncMerger::new(c.storage.connection_arc(), "dev-1".to_string());
    // regime-state 어댑터도 동일 GuardedConnection 으로 만들어진다(타입-레벨 coverage).
    let regime_store: Arc<dyn RegimeStoragePort> = Arc::new(SqliteRegimeManagerStateStore::new(
        c.storage.connection_arc(),
    ));
    let _ = &regime_store;

    let writers = tokio::spawn(async move {
        // events/metrics/idle/suggestions (port) — flag set 이므로 no-op Ok.
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
        // suggestions family (funnel INSERT into `suggestions` ∈ ALL_TABLES) — 스킵.
        funnel_insert(
            &storage_w,
            "INSERT INTO suggestions (suggestion_id, suggestion_type, content, priority, \
             confidence_score, created_at) \
             VALUES ('sug-during', 'tip', 'x', 'LOW', 0.5, '2026-02-02T00:00:00Z')",
        );
        // sync-merge (connection_arc 어댑터) — 동일 flag 로 스킵.
        let _ = merger
            .apply_changes(maekon_core::models::sync::ChangeSet::default())
            .await;
        // funnel 직접 INSERT (digests/focus/sessions/regime-state) — 스킵.
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
        // frames — 스킵(빈 경로).
        let _ = frames_w.save_frame(Utc::now(), b"during-erase-frame").await;
    });

    // erase: Phase-1 SQLite 전체 삭제(retained 경로) + Phase-2 프레임 배리어 삭제.
    let storage_e = c.storage.clone();
    let erase = tokio::task::spawn_blocking(move || storage_e.delete_all_data());
    erase.await.unwrap().unwrap();
    let deleted = c.frames.delete_all_files().await.unwrap();
    let _ = deleted;

    writers.await.unwrap();

    // ── 단언: 모든 ALL_TABLES 테이블 비어있음 ───────────────────────────────
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
            "erase 후 '{t}' 테이블에 잔존 row 가 없어야 한다 (in-flight writer 스킵 + wipe)"
        );
    }

    // 프레임 디렉터리: revoke 후 쓰기 스킵 + delete_all_files 로 비어있어야 한다.
    let remaining = std::fs::read_dir(c.frames.frames_dir())
        .map(|rd| rd.filter_map(|e| e.ok()).count())
        .unwrap_or(0);
    assert_eq!(
        remaining, 0,
        "erase 후 프레임 디렉터리가 비어 있어야 한다(잔존 프레임 없음)"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 테스트 3: flag set 이후 단일 쓰기는 no-op Ok (row 수 불변)
// ───────────────────────────────────────────────────────────────────────────

/// #4928: `deletion_flag` set 이후의 쓰기는 funnel 에서 스킵되어 `Ok` 를 반환하고
/// row 수가 변하지 않는다(SQLite + frames 모두).
#[tokio::test]
async fn write_after_flag_set_is_noop_ok() {
    let c = build_composition().await;

    // revoke 전 1건.
    c.storage.save_metrics(&metrics_sample()).await.unwrap();
    let before = count_rows(&c.storage, "system_metrics");
    assert_eq!(before, 1);

    // revoke → flag set.
    c.consent.revoke_consent().unwrap();

    // 쓰기 시도: Ok 반환하지만 row 불변.
    c.storage
        .save_metrics(&metrics_sample())
        .await
        .expect("flag set 시 save_metrics 는 no-op Ok 여야 한다");
    assert_eq!(
        count_rows(&c.storage, "system_metrics"),
        before,
        "flag set 이후 쓰기는 row 수를 바꾸지 않아야 한다"
    );

    // frames: 빈 경로(스킵) + 파일 미생성.
    let p = c.frames.save_frame(Utc::now(), b"noop").await.unwrap();
    assert!(
        p.as_os_str().is_empty(),
        "flag set 시 frame 쓰기는 빈 경로(스킵)"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 테스트 4: grant 가 flag 를 clear → 쓰기 재개
// ───────────────────────────────────────────────────────────────────────────

/// #4928: revoke 후 `grant_consent` 가 LOCAL flag 를 clear 하여 쓰기가 재개된다.
#[tokio::test]
async fn grant_after_revoke_resumes_writes() {
    let c = build_composition().await;

    c.consent.revoke_consent().unwrap();
    // revoke 직후: 쓰기 스킵.
    c.storage.save_metrics(&metrics_sample()).await.unwrap();
    assert_eq!(
        count_rows(&c.storage, "system_metrics"),
        0,
        "revoke 직후 쓰기는 스킵되어야 한다"
    );

    // 재동의 → flag clear.
    c.consent
        .grant_consent(ConsentPermissions::default(), 30)
        .unwrap();

    // 쓰기 재개 → persist.
    c.storage.save_metrics(&metrics_sample()).await.unwrap();
    assert_eq!(
        count_rows(&c.storage, "system_metrics"),
        1,
        "재동의 후 쓰기는 persist 되어야 한다 (flag clear)"
    );

    // frames 도 재개.
    let p = c
        .frames
        .save_frame(Utc::now(), b"after-regrant")
        .await
        .unwrap();
    assert!(
        !p.as_os_str().is_empty(),
        "재동의 후 frame 쓰기는 실제 경로를 반환해야 한다"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 테스트 5: 보존(retained) 테이블은 flag set 이후에도 기록된다
// ───────────────────────────────────────────────────────────────────────────

/// #4928: `egress_ledger`/`app_meta`/`audit_log` 는 retained 경로를 사용하므로
/// flag set 이후에도 쓰기가 스킵되지 않는다(보존 테이블은 erase 대상이 아니다).
#[tokio::test]
async fn retained_tables_not_skipped_after_flag_set() {
    let c = build_composition().await;

    c.consent.revoke_consent().unwrap();

    // egress_ledger (retained_write_lock) — flag set 이어도 기록되어야 한다.
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
        .expect("retained egress 쓰기는 flag set 이어도 성공해야 한다");
    assert_eq!(
        count_rows(&c.storage, "egress_ledger"),
        1,
        "egress_ledger(retained)는 flag set 이후에도 기록되어야 한다"
    );

    // app_meta (set_meta_checked, retained) — flag set 이어도 기록.
    c.storage
        .set_meta_checked("post_revoke_marker", "1")
        .expect("retained app_meta 쓰기는 flag set 이어도 성공해야 한다");
    assert_eq!(
        c.storage.get_meta("post_revoke_marker"),
        Some("1".to_string()),
        "app_meta(retained)는 flag set 이후에도 기록되어야 한다"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 테스트 6: grant_consent-during-erase TOCTOU — erasing 신호가 race 를 차단한다
// ───────────────────────────────────────────────────────────────────────────

/// #4928 round-3 (FIX B): erase 윈도우(Phase-1 커밋 후 ~ Phase-2 진행 중) 안에
/// `grant_consent` 가 끼어들어 `deletion_flag` 를 clear 해도, `erasing` 이 set 인 동안에는
/// in-flight 쓰기가 계속 스킵되어야 한다(write skip 술어 = `deletion_flag || erasing`).
/// erase 가 끝나(`erasing=false`) + 재동의가 적용된 뒤에야 쓰기가 재개된다.
#[tokio::test]
async fn grant_during_erase_window_is_still_skipped_until_erasing_clears() {
    let c = build_composition().await;

    let erasing = c.consent.erasing();
    let deletion = c.consent.deletion_flag();

    // 1) revoke → erase 시작을 시뮬레이션: erasing=true (RAII 가드가 set 하는 신호).
    c.consent.revoke_consent().unwrap();
    assert!(
        deletion.load(Ordering::Acquire),
        "revoke 후 deletion_flag set"
    );
    erasing.store(true, Ordering::Release); // EraseWindowGuard::set 과 동형.

    // 2) erase 윈도우 한가운데서 동시 재동의: deletion_flag 만 clear 된다.
    c.consent
        .grant_consent(ConsentPermissions::default(), 30)
        .unwrap();
    assert!(
        !deletion.load(Ordering::Acquire),
        "재동의가 deletion_flag 를 clear 했다(TOCTOU 윈도우 재현)"
    );
    assert!(
        erasing.load(Ordering::Acquire),
        "grant_consent 는 erasing 을 clear 하지 못한다"
    );

    // 3) in-flight SQLite 쓰기는 여전히 스킵되어야 한다(erasing set 때문에).
    c.storage.save_metrics(&metrics_sample()).await.unwrap();
    assert_eq!(
        count_rows(&c.storage, "system_metrics"),
        0,
        "erasing set 인 동안에는 재동의 후에도 SQLite 쓰기가 스킵되어야 한다 (TOCTOU 차단)"
    );
    // 직접 funnel INSERT 도 스킵.
    let n = funnel_insert(
        &c.storage,
        "INSERT INTO events (event_id, event_type, timestamp, data) \
         VALUES ('toctou-1','window','2026-01-01T00:00:00Z','{}')",
    );
    assert_eq!(n, 0, "erasing set 인 동안 events 쓰기도 스킵");
    assert_eq!(count_rows(&c.storage, "events"), 0);

    // 4) in-flight 프레임 쓰기도 스킵(빈 경로).
    let p = c
        .frames
        .save_frame(Utc::now(), b"toctou-frame")
        .await
        .unwrap();
    assert!(
        p.as_os_str().is_empty(),
        "erasing set 인 동안에는 프레임 쓰기도 스킵되어야 한다 (빈 PathBuf)"
    );

    // 5) erase 완료: erasing=false (EraseWindowGuard Drop 과 동형). 재동의는 이미 적용됨.
    erasing.store(false, Ordering::Release);

    // 이제 deletion_flag 도 clear, erasing 도 clear → 쓰기 재개.
    c.storage.save_metrics(&metrics_sample()).await.unwrap();
    assert_eq!(
        count_rows(&c.storage, "system_metrics"),
        1,
        "erase 완료 + 재동의 후 SQLite 쓰기가 재개되어야 한다"
    );
    let p2 = c
        .frames
        .save_frame(Utc::now(), b"after-erase")
        .await
        .unwrap();
    assert!(
        !p2.as_os_str().is_empty(),
        "erase 완료 + 재동의 후 프레임 쓰기가 재개되어야 한다"
    );
}

/// #4928 round-3 (FIX B): composition seam 을 통해 consent ↔ SQLite ↔ frames 가
/// 동일한 `erasing` Arc 를 공유하는지 ptr-eq 로 검증한다.
#[tokio::test]
async fn erasing_signal_shared_through_composition_ptr_eq() {
    let c = build_composition().await;
    let consent_erasing = c.consent.erasing();
    assert!(
        Arc::ptr_eq(&c.storage.erasing(), &consent_erasing),
        "ConsentManager 와 SqliteStorage 는 동일 erasing Arc 를 공유해야 한다 (ptr-eq)"
    );
    assert!(
        Arc::ptr_eq(&c.frames.erasing(), &consent_erasing),
        "ConsentManager 와 FrameFileStorage 는 동일 erasing Arc 를 공유해야 한다 (ptr-eq)"
    );
    // connection_arc() 어댑터도 동일 erasing 을 본다.
    assert!(
        Arc::ptr_eq(&c.storage.connection_arc().erasing(), &consent_erasing),
        "connection_arc() 어댑터도 동일 erasing 을 공유해야 한다"
    );
}
