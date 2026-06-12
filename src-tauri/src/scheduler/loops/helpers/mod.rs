// OOS-TBD: ADR-013 file split (cycle 37+) — LOC: 508
//! Helper functions for the monitor loop, split into cohesive sub-modules.

mod analysis;
mod audit;
mod capture;
mod coaching;
mod idle;
mod overlay;

// Re-export the full public surface previously provided by the flat helpers.rs.
pub(super) use analysis::enqueue_and_surface;
pub(super) use analysis::handle_event_analysis;
pub(crate) use audit::record_to_segment_summary;
pub(super) use audit::{audit_consent_and_pii_changes, build_segment_stats_snapshot};
pub(super) use capture::{
    enforce_frame_retention, handle_frame_capture, redact_window_title, FRAME_RETENTION_INTERVAL,
};
pub(super) use coaching::{build_personalization_prompt, COACHING_SYSTEM_PROMPT};
pub(super) use idle::handle_idle_tick;
pub(super) use overlay::{
    emit_heatmap_and_goals, emit_pointer_context_highlight, PointerContextEmitterState,
};

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::activity::{IdlePeriod, ProcessSnapshot, SessionStats};
    use maekon_core::models::frame::{FrameMetadata, ProcessedFrame};
    use maekon_core::models::system::SystemMetrics;
    use maekon_core::ports::storage::MetricsStorage;
    use maekon_monitor::input_activity::InputActivityCollector;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use tokio::sync::broadcast;

    use chrono::Utc;
    use maekon_api_contracts::stream::RealtimeEvent;
    use maekon_core::models::frame::ImagePayload;
    use maekon_core::ports::vision::{CaptureRequest, FrameProcessor};
    use std::sync::Arc;

    // ── Minimal mock: implements SchedulerStorage + MetricsStorage ────────
    //
    // Only `start_idle_period` and `end_idle_period` are exercised by
    // `handle_idle_tick`. All other methods panic with `unimplemented!` to
    // surface accidental calls clearly in test output.
    #[derive(Default)]
    struct MockSchedulerStorage {
        saved_ocr_texts: Mutex<Vec<Option<String>>>,
        saved_metadata: Mutex<Vec<FrameMetadata>>,
        incremented_frames: AtomicU64,
    }

    #[async_trait::async_trait]
    impl MetricsStorage for MockSchedulerStorage {
        async fn save_metrics(
            &self,
            _: &SystemMetrics,
        ) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call save_metrics")
        }

        async fn get_metrics(
            &self,
            _: chrono::DateTime<chrono::Utc>,
            _: chrono::DateTime<chrono::Utc>,
            _: usize,
        ) -> Result<Vec<SystemMetrics>, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call get_metrics")
        }

        async fn aggregate_hourly_metrics(
            &self,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call aggregate_hourly_metrics")
        }

        async fn cleanup_old_metrics(
            &self,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<usize, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call cleanup_old_metrics")
        }

        async fn save_process_snapshot(
            &self,
            _: &ProcessSnapshot,
        ) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call save_process_snapshot")
        }

        async fn get_process_snapshots(
            &self,
            _: chrono::DateTime<chrono::Utc>,
            _: chrono::DateTime<chrono::Utc>,
            _: usize,
        ) -> Result<Vec<ProcessSnapshot>, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call get_process_snapshots")
        }

        async fn cleanup_old_process_snapshots(
            &self,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<usize, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call cleanup_old_process_snapshots")
        }

        /// Returns a fixed id (1) so `idle_tracker.set_idle_period_id` gets a
        /// valid value without touching real storage.
        async fn start_idle_period(
            &self,
            _start_time: chrono::DateTime<chrono::Utc>,
        ) -> Result<i64, maekon_core::error::CoreError> {
            Ok(1)
        }

        async fn end_idle_period(
            &self,
            _id: i64,
            _end_time: chrono::DateTime<chrono::Utc>,
        ) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }

        async fn get_ongoing_idle_period(
            &self,
        ) -> Result<Option<(i64, IdlePeriod)>, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call get_ongoing_idle_period")
        }

        async fn get_idle_periods(
            &self,
            _: chrono::DateTime<chrono::Utc>,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<Vec<IdlePeriod>, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call get_idle_periods")
        }

        async fn cleanup_old_idle_periods(
            &self,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<usize, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call cleanup_old_idle_periods")
        }

        async fn upsert_session(
            &self,
            _: &SessionStats,
        ) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call upsert_session")
        }

        async fn get_session(
            &self,
            _: &str,
        ) -> Result<Option<SessionStats>, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call get_session")
        }

        async fn end_session(
            &self,
            _: &str,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call end_session")
        }

        async fn increment_session_counters(
            &self,
            _session_id: &str,
            _events: u64,
            frames: u64,
            _idle_secs: u64,
        ) -> Result<(), maekon_core::error::CoreError> {
            self.incremented_frames.fetch_add(frames, Ordering::Relaxed);
            Ok(())
        }
    }

    impl crate::scheduler::config::SchedulerStorage for MockSchedulerStorage {
        fn save_frame_metadata_with_bounds(
            &self,
            metadata: &maekon_core::models::frame::FrameMetadata,
            _: Option<&str>,
            ocr_text: Option<&str>,
            _: Option<&maekon_core::models::context::WindowBounds>,
        ) -> Result<i64, maekon_core::error::CoreError> {
            let mut metadata_rows = self.saved_metadata.lock().expect("metadata lock poisoned");
            metadata_rows.push(metadata.clone());
            let row_id = metadata_rows.len() as i64;
            self.saved_ocr_texts
                .lock()
                .expect("ocr lock poisoned")
                .push(ocr_text.map(str::to_string));
            Ok(row_id)
        }

        fn has_recent_server_suggestions(
            &self,
            _: u64,
        ) -> Result<bool, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call has_recent_server_suggestions")
        }

        fn list_weekly_digests(
            &self,
            _: usize,
        ) -> Result<
            Vec<maekon_core::models::weekly_digest::WeeklyDigest>,
            maekon_core::error::CoreError,
        > {
            unimplemented!("handle_idle_tick should not call list_weekly_digests")
        }

        fn save_weekly_digest(
            &self,
            _: &maekon_core::models::weekly_digest::WeeklyDigest,
        ) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call save_weekly_digest")
        }

        fn list_segments_between(
            &self,
            _: chrono::DateTime<chrono::Utc>,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<
            Vec<maekon_core::models::tiered_memory::SegmentSummary>,
            maekon_core::error::CoreError,
        > {
            unimplemented!("handle_idle_tick should not call list_segments_between")
        }

        fn enforce_segment_retention(
            &self,
            _: u32,
        ) -> Result<usize, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call enforce_segment_retention")
        }

        fn enforce_digest_retention(&self, _: u32) -> Result<usize, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call enforce_digest_retention")
        }

        fn get_daily_digest(
            &self,
            _: &str,
        ) -> Result<
            Option<maekon_core::models::daily_digest::DailyDigest>,
            maekon_core::error::CoreError,
        > {
            unimplemented!("handle_idle_tick should not call get_daily_digest")
        }

        fn save_daily_digest(
            &self,
            _: &maekon_core::models::daily_digest::DailyDigest,
        ) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call save_daily_digest")
        }

        fn get_segments_for_date(
            &self,
            _: &str,
        ) -> Result<
            Vec<maekon_core::models::storage_records::SegmentSummaryRecord>,
            maekon_core::error::CoreError,
        > {
            unimplemented!("handle_idle_tick should not call get_segments_for_date")
        }

        fn save_gui_interaction(
            &self,
            _: &maekon_core::models::storage_records::NewGuiInteraction<'_>,
        ) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call save_gui_interaction")
        }

        fn enforce_all_retention(&self) -> Result<u64, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call enforce_all_retention")
        }

        fn gc_sync_tombstones(
            &self,
            _data_retention_days: u32,
        ) -> Result<usize, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call gc_sync_tombstones")
        }

        fn wal_checkpoint_passive(&self) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call wal_checkpoint_passive")
        }

        fn maybe_vacuum(&self, _: u64) -> Result<bool, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call maybe_vacuum")
        }

        fn fts_merge(&self, _: u32) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call fts_merge")
        }

        fn fts_optimize(&self) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call fts_optimize")
        }

        fn run_analyze(&self) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call run_analyze")
        }

        fn record_egress(
            &self,
            _: &maekon_core::models::storage_records::EgressLedgerRecord,
        ) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call record_egress")
        }
    }

    struct StaticFrameProcessor {
        frame: ProcessedFrame,
    }

    #[async_trait::async_trait]
    impl FrameProcessor for StaticFrameProcessor {
        async fn capture_and_process(
            &self,
            _: &CaptureRequest,
        ) -> Result<ProcessedFrame, maekon_core::error::CoreError> {
            Ok(self.frame.clone())
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn crt_prv_cap_004_ocr_text_sanitized_before_storage() {
        let raw_ocr =
            "contact user@example.com card 4111 1111 1111 1111 IBAN DE89370400440532013000";
        let frame = ProcessedFrame {
            metadata: FrameMetadata {
                timestamp: Utc::now(),
                trigger_type: "active_window_change".to_string(),
                app_name: "Notes".to_string(),
                window_title: "meeting notes".to_string(),
                resolution: (1280, 720),
                importance: 1.0,
                monitor_id: None,
                app_bundle_id: None,
            },
            image_payload: Some(ImagePayload::Full {
                data: "not-decoded-without-frame-storage".to_string(),
                format: "webp".to_string(),
                ocr_text: Some(raw_ocr.to_string()),
            }),
            ocr_regions: Vec::new(),
            raw_rgba: None,
        };
        let processor: Arc<dyn FrameProcessor> = Arc::new(StaticFrameProcessor { frame });
        let storage = Arc::new(MockSchedulerStorage::default());
        let sqlite: Arc<dyn crate::scheduler::config::SchedulerStorage> = storage.clone();
        let capture_req = CaptureRequest {
            trigger_type: "active_window_change".to_string(),
            importance: 1.0,
            app_name: "Notes".to_string(),
            window_title: "meeting notes".to_string(),
            monitor_id: None,
            app_bundle_id: None,
            window_bounds: None,
            screen_scale_factor: None,
        };

        let (ocr_hint, _, _) = handle_frame_capture(
            &capture_req,
            &processor,
            &None,
            &sqlite,
            "test-session",
            maekon_core::config::PiiFilterLevel::Strict,
            // ocr_processing 동의 부여됨 → OCR 텍스트 경로 활성 (기존 sanitize 동작 유지).
            true,
            &None,
        )
        .await;

        assert_eq!(ocr_hint.as_deref(), Some(raw_ocr));

        let saved = storage
            .saved_ocr_texts
            .lock()
            .expect("ocr lock poisoned")
            .first()
            .cloned()
            .flatten()
            .expect("sanitized ocr row");
        assert!(saved.contains("[EMAIL]"));
        assert!(saved.contains("[CARD]"));
        assert!(saved.contains("[IBAN]"));
        assert!(!saved.contains("user@example.com"));
        assert!(!saved.contains("4111 1111 1111 1111"));
        assert!(!saved.contains("DE89370400440532013000"));
        assert_eq!(storage.incremented_frames.load(Ordering::Relaxed), 1);
    }

    /// 주어진 OCR 텍스트를 가진 Full 페이로드 ProcessedFrame 을 만든다 (OCR 게이트 테스트용).
    fn frame_with_ocr(raw_ocr: &str) -> ProcessedFrame {
        ProcessedFrame {
            metadata: FrameMetadata {
                timestamp: Utc::now(),
                trigger_type: "active_window_change".to_string(),
                app_name: "Notes".to_string(),
                window_title: "meeting notes".to_string(),
                resolution: (1280, 720),
                importance: 1.0,
                monitor_id: None,
                app_bundle_id: None,
            },
            image_payload: Some(ImagePayload::Full {
                data: "not-decoded-without-frame-storage".to_string(),
                format: "webp".to_string(),
                ocr_text: Some(raw_ocr.to_string()),
            }),
            ocr_regions: vec![maekon_core::models::frame::OcrRegion {
                text: raw_ocr.to_string(),
                bbox: maekon_core::models::frame::BoundingBox {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                },
                confidence: 0.9,
            }],
            raw_rgba: None,
        }
    }

    /// Own-field gate (#4802): ocr_processing 동의가 없으면 OCR 텍스트/영역이 폐기되어야 한다.
    /// 프레임은 캡처되지만(frame counter 증가) OCR 힌트는 None, 영역은 비고,
    /// frames.ocr_text 에 저장되는 값도 None 이어야 한다 (텍스트 경로 전체 차단).
    #[tokio::test]
    async fn ocr_not_extracted_when_own_field_denied() {
        let raw_ocr = "contact user@example.com";
        let processor: Arc<dyn FrameProcessor> = Arc::new(StaticFrameProcessor {
            frame: frame_with_ocr(raw_ocr),
        });
        let storage = Arc::new(MockSchedulerStorage::default());
        let sqlite: Arc<dyn crate::scheduler::config::SchedulerStorage> = storage.clone();
        let capture_req = CaptureRequest {
            trigger_type: "active_window_change".to_string(),
            importance: 1.0,
            app_name: "Notes".to_string(),
            window_title: "meeting notes".to_string(),
            monitor_id: None,
            app_bundle_id: None,
            window_bounds: None,
            screen_scale_factor: None,
        };

        let (ocr_hint, regions, _) = handle_frame_capture(
            &capture_req,
            &processor,
            &None,
            &sqlite,
            "test-session",
            maekon_core::config::PiiFilterLevel::Strict,
            // ocr_processing 동의 미부여 → OCR 텍스트 경로 전체 차단.
            false,
            &None,
        )
        .await;

        assert!(
            ocr_hint.is_none(),
            "ocr_processing 미부여 시 OCR 힌트는 None"
        );
        assert!(
            regions.is_empty(),
            "ocr_processing 미부여 시 OCR 영역은 비어야 함"
        );
        // 프레임 자체는 캡처/저장됨 (screen_capture 동의 경로).
        assert_eq!(storage.incremented_frames.load(Ordering::Relaxed), 1);
        // frames.ocr_text 에 저장된 값은 None (텍스트가 새지 않음).
        let saved = storage
            .saved_ocr_texts
            .lock()
            .expect("ocr lock poisoned")
            .first()
            .cloned()
            .flatten();
        assert!(
            saved.is_none(),
            "ocr_processing 미부여 시 frames.ocr_text 는 None 이어야 함 (텍스트 누출 없음)"
        );
    }

    /// Own-field gate (#4802): ocr_processing 동의가 있으면 OCR 텍스트/영역이 추출되어야 한다.
    #[tokio::test]
    async fn ocr_extracted_when_own_field_granted() {
        let raw_ocr = "meeting agenda 2026";
        let processor: Arc<dyn FrameProcessor> = Arc::new(StaticFrameProcessor {
            frame: frame_with_ocr(raw_ocr),
        });
        let storage = Arc::new(MockSchedulerStorage::default());
        let sqlite: Arc<dyn crate::scheduler::config::SchedulerStorage> = storage.clone();
        let capture_req = CaptureRequest {
            trigger_type: "active_window_change".to_string(),
            importance: 1.0,
            app_name: "Notes".to_string(),
            window_title: "meeting notes".to_string(),
            monitor_id: None,
            app_bundle_id: None,
            window_bounds: None,
            screen_scale_factor: None,
        };

        let (ocr_hint, regions, _) = handle_frame_capture(
            &capture_req,
            &processor,
            &None,
            &sqlite,
            "test-session",
            maekon_core::config::PiiFilterLevel::Strict,
            // ocr_processing 동의 부여 → OCR 텍스트/영역 추출됨.
            true,
            &None,
        )
        .await;

        assert_eq!(
            ocr_hint.as_deref(),
            Some(raw_ocr),
            "동의 부여 시 OCR 힌트 반환"
        );
        assert_eq!(regions.len(), 1, "동의 부여 시 OCR 영역 반환");
    }

    /// Own-field gate (#4802): window_title_collection 동의가 없으면(=false) 윈도우
    /// 제목이 redact(빈 문자열)되어야 한다 — monitor 루프가 다운스트림에 넘기는 값.
    #[test]
    fn window_title_not_collected_with_only_monitoring_bundle() {
        let redacted = redact_window_title("secret document.docx".to_string(), false);
        assert_eq!(
            redacted, "",
            "window_title_collection 미부여 시 제목은 빈 문자열로 redact"
        );
    }

    /// Own-field gate (#4802): window_title_collection 동의가 있으면(=true) 원본 제목을
    /// 그대로 보존해야 한다.
    #[test]
    fn window_title_collected_when_own_field_granted() {
        let title = "meeting notes — agenda".to_string();
        let kept = redact_window_title(title.clone(), true);
        assert_eq!(
            kept, title,
            "window_title_collection 부여 시 원본 제목 보존"
        );
    }

    /// Verifies the publisher-side edge-detection invariant (spec §U2 I2):
    /// `handle_idle_tick` emits exactly one `RealtimeEvent::Idle(is_idle=true)`
    /// on the Active→Idle transition, and suppresses duplicate emission on the
    /// subsequent mid-Idle tick (Idle→Idle).
    ///
    /// Uses `threshold_secs=0` so `get_idle_time() >= 0` always yields
    /// `IdleState::Idle`, making the test deterministic regardless of actual
    /// platform idle time at test runtime.
    #[tokio::test]
    async fn handle_idle_tick_emits_on_edge_only() {
        let sqlite: Arc<dyn crate::scheduler::config::SchedulerStorage> =
            Arc::new(MockSchedulerStorage::default());
        // threshold=0 → check_idle() always returns Idle (any idle_secs ≥ 0).
        let mut idle_tracker = maekon_monitor::idle::IdleTracker::new(Some(0));
        let input_collector = InputActivityCollector::new();
        let (tx, mut rx) = broadcast::channel::<RealtimeEvent>(16);
        let event_tx: Option<broadcast::Sender<RealtimeEvent>> = Some(tx);

        // ── Call 1: Active→Idle edge ─────────────────────────────────────
        idle::handle_idle_tick(
            &mut idle_tracker,
            &sqlite,
            &None,
            &input_collector,
            0,
            false,
            &event_tx,
        )
        .await;

        let first = rx
            .try_recv()
            .expect("expected one Idle event on Active→Idle edge");
        match first {
            RealtimeEvent::Idle(update) => {
                assert!(
                    update.is_idle,
                    "first emission must carry is_idle=true (Active→Idle edge)"
                );
            }
            other => panic!("expected RealtimeEvent::Idle, got {other:?}"),
        }

        // ── Call 2: mid-Idle (Idle→Idle) — no second emission ───────────
        idle::handle_idle_tick(
            &mut idle_tracker,
            &sqlite,
            &None,
            &input_collector,
            0,
            false,
            &event_tx,
        )
        .await;

        match rx.try_recv() {
            Err(broadcast::error::TryRecvError::Empty) => {
                // Correct: mid-Idle tick must not emit.
            }
            Ok(extra) => panic!("unexpected second emission on mid-Idle tick: {extra:?}"),
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                panic!("receiver lagged by {n} messages — channel capacity too small?")
            }
            Err(broadcast::error::TryRecvError::Closed) => {
                panic!("broadcast channel closed unexpectedly")
            }
        }
    }

    /// Bonus: verifies that Active→Active (mid-Active) ticks are also suppressed.
    ///
    /// Uses `threshold_secs=u64::MAX` so check_idle always returns Active
    /// (no idle_secs value can reach u64::MAX). A fresh tracker starts with
    /// previous_state=Active, so two consecutive ticks are both Active→Active:
    /// neither should emit.
    ///
    /// Note: The Idle→Active edge is not covered here because IdleTracker does
    /// not expose a test-only setter for `previous_state`, making it impossible
    /// to deterministically prime a MAX-threshold tracker to the Idle state
    /// without modifying the tracker itself. That edge is exercised end-to-end
    /// by `subscribe_events_streams_idle_on_edge_only` in the gRPC integration
    /// suite (`grpc_dashboard_integration.rs`).
    #[tokio::test]
    async fn handle_idle_tick_suppresses_mid_active_tick() {
        let sqlite: Arc<dyn crate::scheduler::config::SchedulerStorage> =
            Arc::new(MockSchedulerStorage::default());
        // threshold=MAX → check_idle always returns Active.
        let mut idle_tracker = maekon_monitor::idle::IdleTracker::new(Some(u64::MAX));
        let input_collector = InputActivityCollector::new();
        let (tx, mut rx) = broadcast::channel::<RealtimeEvent>(16);
        let event_tx: Option<broadcast::Sender<RealtimeEvent>> = Some(tx);

        // ── Call 1: Active→Active — no emit ──────────────────────────────
        idle::handle_idle_tick(
            &mut idle_tracker,
            &sqlite,
            &None,
            &input_collector,
            0,
            false,
            &event_tx,
        )
        .await;

        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "Active→Active (call 1) must not emit"
        );

        // ── Call 2: Active→Active again — still no emit ──────────────────
        idle::handle_idle_tick(
            &mut idle_tracker,
            &sqlite,
            &None,
            &input_collector,
            0,
            false,
            &event_tx,
        )
        .await;

        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "Active→Active (call 2) must not emit"
        );
    }
}
