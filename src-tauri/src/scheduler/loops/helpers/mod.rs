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
// #7914: uniform learned-relevance gating seam shared by every LOCAL producer.
// Producers name only the `relevance_gates` builder; the returned
// `RelevanceGates` value flows to `enqueue_and_surface` via type inference, so
// the type itself does not need re-exporting here.
pub(super) use analysis::relevance_gates;
pub(crate) use audit::record_to_segment_summary;
pub(super) use audit::{audit_consent_and_pii_changes, build_segment_stats_snapshot};
pub(super) use capture::{
    enforce_frame_retention, handle_frame_capture, redact_window_title, FRAME_RETENTION_INTERVAL,
};
pub(super) use coaching::{build_personalization_prompt, COACHING_SYSTEM_PROMPT};
pub(super) use idle::{handle_idle_tick, IdleTickServices};
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
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Mutex;
    use tokio::sync::broadcast;

    use chrono::Utc;
    use maekon_api_contracts::stream::RealtimeEvent;
    use maekon_core::error::CoreError;
    use maekon_core::models::frame::ImagePayload;
    use maekon_core::models::suggestion::Suggestion;
    use maekon_core::ports::focus_storage::FocusStorage;
    use maekon_core::ports::notifier::DesktopNotifier;
    use maekon_core::ports::vision::{CaptureRequest, FrameProcessor};
    use maekon_storage::sqlite::SqliteStorage;
    use std::sync::Arc;
    use tempfile::TempDir;

    // ── Minimal mock: implements SchedulerStorage + MetricsStorage ────────
    //
    // Only `start_idle_period` and `end_idle_period` are exercised by
    // `handle_idle_tick`. All other methods panic with `unimplemented!` to
    // surface accidental calls clearly in test output.
    #[derive(Default)]
    struct MockSchedulerStorage {
        saved_ocr_texts: Mutex<Vec<Option<String>>>,
        saved_metadata: Mutex<Vec<FrameMetadata>>,
        // #6133: capture the window bounds passed to the offloaded write so the
        // regression test can assert owned data survives the spawn_blocking move.
        saved_bounds: Mutex<Vec<Option<maekon_core::models::context::WindowBounds>>>,
        incremented_frames: AtomicU64,
    }

    struct NoopDesktopNotifier;

    #[async_trait::async_trait]
    impl DesktopNotifier for NoopDesktopNotifier {
        async fn show_suggestion(&self, _: &Suggestion) -> Result<(), CoreError> {
            Ok(())
        }

        async fn show_notification(&self, _: &str, _: &str) -> Result<(), CoreError> {
            Ok(())
        }

        async fn show_error(&self, _: &str) -> Result<(), CoreError> {
            Ok(())
        }
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

        async fn cleanup_old_hourly_metrics(
            &self,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<usize, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call cleanup_old_hourly_metrics")
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

    impl crate::scheduler::SchedulerStorage for MockSchedulerStorage {
        fn save_frame_metadata_with_bounds(
            &self,
            metadata: &maekon_core::models::frame::FrameMetadata,
            _: Option<&str>,
            ocr_text: Option<&str>,
            bounds: Option<&maekon_core::models::context::WindowBounds>,
        ) -> Result<i64, maekon_core::error::CoreError> {
            let mut metadata_rows = self.saved_metadata.lock().expect("metadata lock poisoned");
            metadata_rows.push(metadata.clone());
            let row_id = metadata_rows.len() as i64;
            self.saved_ocr_texts
                .lock()
                .expect("ocr lock poisoned")
                .push(ocr_text.map(str::to_string));
            self.saved_bounds
                .lock()
                .expect("bounds lock poisoned")
                .push(bounds.cloned());
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

        fn list_daily_digests(
            &self,
            _: usize,
        ) -> Result<
            Vec<maekon_core::models::daily_digest::DailyDigest>,
            maekon_core::error::CoreError,
        > {
            unimplemented!("handle_idle_tick should not call list_daily_digests")
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

        fn has_digest_processing_marker(
            &self,
            _: &str,
            _: &str,
        ) -> Result<bool, maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call has_digest_processing_marker")
        }

        fn save_digest_processing_marker(
            &self,
            _: &str,
            _: &str,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<(), maekon_core::error::CoreError> {
            unimplemented!("handle_idle_tick should not call save_digest_processing_marker")
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

    struct RecordingFrameProcessor {
        frame: ProcessedFrame,
        saw_ocr_permitted: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl FrameProcessor for RecordingFrameProcessor {
        async fn capture_and_process(
            &self,
            capture_request: &CaptureRequest,
        ) -> Result<ProcessedFrame, maekon_core::error::CoreError> {
            self.saw_ocr_permitted
                .store(capture_request.ocr_processing_permitted, Ordering::Relaxed);
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
        let sqlite: Arc<dyn crate::scheduler::SchedulerStorage> = storage.clone();
        let capture_req = CaptureRequest {
            trigger_type: "active_window_change".to_string(),
            importance: 1.0,
            app_name: "Notes".to_string(),
            window_title: "meeting notes".to_string(),
            monitor_id: None,
            app_bundle_id: None,
            window_bounds: None,
            screen_scale_factor: None,
            ocr_processing_permitted: true,
        };

        let (ocr_hint, _, _) = handle_frame_capture(
            &capture_req,
            &processor,
            &None,
            &sqlite,
            "test-session",
            maekon_core::config::PiiFilterLevel::Strict,
            // ocr_processing consent granted -> OCR text path active (existing sanitize behavior preserved).
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

    /// Builds a Full-payload ProcessedFrame carrying the given OCR text (for OCR-gate tests).
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

    /// Own-field gate (#4802): without ocr_processing consent, OCR text/regions must be discarded.
    /// The frame is still captured (frame counter increments) but the OCR hint is None, regions are
    /// empty, and the value stored in frames.ocr_text must also be None (entire text path blocked).
    #[tokio::test]
    async fn ocr_not_extracted_when_own_field_denied() {
        let raw_ocr = "contact user@example.com";
        let processor: Arc<dyn FrameProcessor> = Arc::new(StaticFrameProcessor {
            frame: frame_with_ocr(raw_ocr),
        });
        let storage = Arc::new(MockSchedulerStorage::default());
        let sqlite: Arc<dyn crate::scheduler::SchedulerStorage> = storage.clone();
        let capture_req = CaptureRequest {
            trigger_type: "active_window_change".to_string(),
            importance: 1.0,
            app_name: "Notes".to_string(),
            window_title: "meeting notes".to_string(),
            monitor_id: None,
            app_bundle_id: None,
            window_bounds: None,
            screen_scale_factor: None,
            ocr_processing_permitted: true,
        };

        let (ocr_hint, regions, _) = handle_frame_capture(
            &capture_req,
            &processor,
            &None,
            &sqlite,
            "test-session",
            maekon_core::config::PiiFilterLevel::Strict,
            // ocr_processing consent not granted -> entire OCR text path blocked.
            false,
            &None,
        )
        .await;

        assert!(
            ocr_hint.is_none(),
            "OCR hint must be None when ocr_processing is not granted"
        );
        assert!(
            regions.is_empty(),
            "OCR regions must be empty when ocr_processing is not granted"
        );
        // The frame itself is still captured/stored (screen_capture consent path).
        assert_eq!(storage.incremented_frames.load(Ordering::Relaxed), 1);
        // The value stored in frames.ocr_text is None (text does not leak).
        let saved = storage
            .saved_ocr_texts
            .lock()
            .expect("ocr lock poisoned")
            .first()
            .cloned()
            .flatten();
        assert!(
            saved.is_none(),
            "frames.ocr_text must be None when ocr_processing is not granted (no text leak)"
        );
    }

    /// Own-field gate (#4802): without ocr_processing consent, OCR-only raw
    /// pixels must not leave the frame capture helper.
    #[tokio::test]
    async fn ocr_raw_rgba_not_returned_when_own_field_denied() {
        let mut frame = frame_with_ocr("contact user@example.com");
        frame.raw_rgba = Some(vec![255, 0, 0, 255]);
        let processor: Arc<dyn FrameProcessor> = Arc::new(StaticFrameProcessor { frame });
        let storage = Arc::new(MockSchedulerStorage::default());
        let sqlite: Arc<dyn crate::scheduler::SchedulerStorage> = storage.clone();
        let capture_req = CaptureRequest {
            trigger_type: "active_window_change".to_string(),
            importance: 1.0,
            app_name: "Notes".to_string(),
            window_title: "meeting notes".to_string(),
            monitor_id: None,
            app_bundle_id: None,
            window_bounds: None,
            screen_scale_factor: None,
            ocr_processing_permitted: true,
        };

        let (_, _, raw_rgba) = handle_frame_capture(
            &capture_req,
            &processor,
            &None,
            &sqlite,
            "test-session",
            maekon_core::config::PiiFilterLevel::Strict,
            false,
            &None,
        )
        .await;

        assert!(
            raw_rgba.is_none(),
            "raw RGBA must not be returned when ocr_processing is not granted"
        );
    }

    /// Own-field gate (#4802): scheduler must pass the current ocr_processing
    /// consent decision into the processor before OCR work can start.
    #[tokio::test]
    async fn ocr_processor_request_reflects_own_field_denial() {
        let saw_ocr_permitted = Arc::new(AtomicBool::new(true));
        let processor: Arc<dyn FrameProcessor> = Arc::new(RecordingFrameProcessor {
            frame: frame_with_ocr("contact user@example.com"),
            saw_ocr_permitted: saw_ocr_permitted.clone(),
        });
        let storage = Arc::new(MockSchedulerStorage::default());
        let sqlite: Arc<dyn crate::scheduler::SchedulerStorage> = storage.clone();
        let capture_req = CaptureRequest {
            trigger_type: "active_window_change".to_string(),
            importance: 1.0,
            app_name: "Notes".to_string(),
            window_title: "meeting notes".to_string(),
            monitor_id: None,
            app_bundle_id: None,
            window_bounds: None,
            screen_scale_factor: None,
            ocr_processing_permitted: true,
        };

        handle_frame_capture(
            &capture_req,
            &processor,
            &None,
            &sqlite,
            "test-session",
            maekon_core::config::PiiFilterLevel::Strict,
            false,
            &None,
        )
        .await;

        assert!(
            !saw_ocr_permitted.load(Ordering::Relaxed),
            "processor request must carry ocr_processing_permitted=false"
        );
    }

    /// Own-field gate (#4802): with ocr_processing consent, OCR text/regions must be extracted.
    #[tokio::test]
    async fn ocr_extracted_when_own_field_granted() {
        let raw_ocr = "meeting agenda 2026";
        let processor: Arc<dyn FrameProcessor> = Arc::new(StaticFrameProcessor {
            frame: frame_with_ocr(raw_ocr),
        });
        let storage = Arc::new(MockSchedulerStorage::default());
        let sqlite: Arc<dyn crate::scheduler::SchedulerStorage> = storage.clone();
        let capture_req = CaptureRequest {
            trigger_type: "active_window_change".to_string(),
            importance: 1.0,
            app_name: "Notes".to_string(),
            window_title: "meeting notes".to_string(),
            monitor_id: None,
            app_bundle_id: None,
            window_bounds: None,
            screen_scale_factor: None,
            ocr_processing_permitted: true,
        };

        let (ocr_hint, regions, _) = handle_frame_capture(
            &capture_req,
            &processor,
            &None,
            &sqlite,
            "test-session",
            maekon_core::config::PiiFilterLevel::Strict,
            // ocr_processing consent granted -> OCR text/regions extracted.
            true,
            &None,
        )
        .await;

        assert_eq!(
            ocr_hint.as_deref(),
            Some(raw_ocr),
            "OCR hint returned when consent is granted"
        );
        assert_eq!(
            regions.len(),
            1,
            "OCR regions returned when consent is granted"
        );
    }

    /// #6133 regression: the frame-metadata SQLite write is offloaded to the
    /// blocking pool via `spawn_blocking`. This test asserts the offloaded write
    /// still persists the metadata, OCR text, and (critically) the `window_bounds`
    /// — proving the owned data moved into the closure survives the move and the
    /// FrameUpdate is still emitted after the awaited save succeeds.
    #[tokio::test]
    async fn frame_metadata_write_offloaded_preserves_bounds_and_emits() {
        use maekon_core::models::context::WindowBounds;

        let raw_ocr = "offloaded write payload";
        let processor: Arc<dyn FrameProcessor> = Arc::new(StaticFrameProcessor {
            frame: frame_with_ocr(raw_ocr),
        });
        let storage = Arc::new(MockSchedulerStorage::default());
        let sqlite: Arc<dyn crate::scheduler::SchedulerStorage> = storage.clone();
        let bounds = WindowBounds {
            x: 11,
            y: 22,
            width: 800,
            height: 600,
        };
        let capture_req = CaptureRequest {
            trigger_type: "active_window_change".to_string(),
            importance: 1.0,
            app_name: "Notes".to_string(),
            window_title: "meeting notes".to_string(),
            monitor_id: None,
            app_bundle_id: None,
            window_bounds: Some(bounds),
            screen_scale_factor: None,
            ocr_processing_permitted: true,
        };

        let (tx, mut rx) = broadcast::channel::<RealtimeEvent>(8);
        let event_tx: Option<broadcast::Sender<RealtimeEvent>> = Some(tx);

        handle_frame_capture(
            &capture_req,
            &processor,
            &None,
            &sqlite,
            "test-session",
            maekon_core::config::PiiFilterLevel::Standard,
            true,
            &event_tx,
        )
        .await;

        // The offloaded write persisted exactly one metadata row.
        assert_eq!(
            storage.saved_metadata.lock().expect("metadata lock").len(),
            1,
            "offloaded write must persist the frame metadata"
        );
        // The owned window bounds survived the spawn_blocking move intact.
        let saved_bounds = storage
            .saved_bounds
            .lock()
            .expect("bounds lock")
            .first()
            .cloned()
            .flatten()
            .expect("window bounds row");
        assert_eq!(saved_bounds.x, 11);
        assert_eq!(saved_bounds.y, 22);
        assert_eq!(saved_bounds.width, 800);
        assert_eq!(saved_bounds.height, 600);
        // The OCR text moved into the closure was persisted.
        let saved_ocr = storage
            .saved_ocr_texts
            .lock()
            .expect("ocr lock")
            .first()
            .cloned()
            .flatten()
            .expect("ocr row");
        assert_eq!(saved_ocr, raw_ocr);
        // FrameUpdate is emitted only after the awaited save succeeds.
        match rx.try_recv() {
            Ok(RealtimeEvent::Frame(update)) => {
                assert_eq!(update.app_name, "Notes");
            }
            other => panic!("expected RealtimeEvent::Frame after offloaded save, got {other:?}"),
        }
        assert_eq!(storage.incremented_frames.load(Ordering::Relaxed), 1);
    }

    /// Own-field gate (#4802): without window_title_collection consent (=false), the window title
    /// must be redacted (empty string) — this is the value the monitor loop passes downstream.
    #[test]
    fn window_title_not_collected_with_only_monitoring_bundle() {
        let redacted = redact_window_title("secret document.docx".to_string(), false);
        assert_eq!(
            redacted, "",
            "title must be redacted to an empty string when window_title_collection is not granted"
        );
    }

    /// Own-field gate (#4802): with window_title_collection consent (=true), the original title
    /// must be preserved as-is.
    #[test]
    fn window_title_collected_when_own_field_granted() {
        let title = "meeting notes — agenda".to_string();
        let kept = redact_window_title(title.clone(), true);
        assert_eq!(
            kept, title,
            "original title preserved when window_title_collection is granted"
        );
    }

    #[tokio::test]
    async fn idle_resume_edge_resets_active_focus_session() {
        let temp_dir = TempDir::new().expect("temp dir");
        let storage = Arc::new(
            SqliteStorage::open(&temp_dir.path().join("focus.db"), 30, None)
                .expect("storage creation failed"),
        );
        let focus_storage: Arc<dyn FocusStorage> = storage.clone();
        let focus = Arc::new(
            maekon_analysis::focus_analyzer::FocusAnalyzer::with_defaults(
                focus_storage,
                Arc::new(NoopDesktopNotifier),
            ),
        );
        let sqlite: Arc<dyn crate::scheduler::SchedulerStorage> =
            Arc::new(MockSchedulerStorage::default());
        let mut idle_tracker = maekon_monitor::idle::IdleTracker::new(Some(u64::MAX));

        focus.on_app_switch("Visual Studio Code").await;

        let before = storage
            .list_work_sessions("1970-01-01", "9999-12-31", 10)
            .expect("list work sessions before resume");
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].state, "active");

        let _ =
            idle::handle_idle_resume_edge(&mut idle_tracker, &sqlite, &None, &Some(focus)).await;

        let after = storage
            .list_work_sessions("1970-01-01", "9999-12-31", 10)
            .expect("list work sessions after resume");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].state, "completed");
        assert!(
            after[0].ended_at.is_some(),
            "idle resume must close the active deep-work session"
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
        let sqlite: Arc<dyn crate::scheduler::SchedulerStorage> =
            Arc::new(MockSchedulerStorage::default());
        // threshold=0 → check_idle() always returns Idle (any idle_secs ≥ 0).
        let mut idle_tracker = maekon_monitor::idle::IdleTracker::new(Some(0));
        let input_collector = InputActivityCollector::new();
        let (tx, mut rx) = broadcast::channel::<RealtimeEvent>(16);
        let event_tx: Option<broadcast::Sender<RealtimeEvent>> = Some(tx);

        // ── Call 1: Active→Idle edge ─────────────────────────────────────
        idle::handle_idle_tick(
            &mut idle_tracker,
            IdleTickServices {
                sqlite: &sqlite,
                notif: &None,
                focus: &None,
                input_collector: &input_collector,
                event_tx: &event_tx,
            },
            0,
            false,
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
            IdleTickServices {
                sqlite: &sqlite,
                notif: &None,
                focus: &None,
                input_collector: &input_collector,
                event_tx: &event_tx,
            },
            0,
            false,
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
        let sqlite: Arc<dyn crate::scheduler::SchedulerStorage> =
            Arc::new(MockSchedulerStorage::default());
        // threshold=MAX → check_idle always returns Active.
        let mut idle_tracker = maekon_monitor::idle::IdleTracker::new(Some(u64::MAX));
        let input_collector = InputActivityCollector::new();
        let (tx, mut rx) = broadcast::channel::<RealtimeEvent>(16);
        let event_tx: Option<broadcast::Sender<RealtimeEvent>> = Some(tx);

        // ── Call 1: Active→Active — no emit ──────────────────────────────
        idle::handle_idle_tick(
            &mut idle_tracker,
            IdleTickServices {
                sqlite: &sqlite,
                notif: &None,
                focus: &None,
                input_collector: &input_collector,
                event_tx: &event_tx,
            },
            0,
            false,
        )
        .await;

        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "Active→Active (call 1) must not emit"
        );

        // ── Call 2: Active→Active again — still no emit ──────────────────
        idle::handle_idle_tick(
            &mut idle_tracker,
            IdleTickServices {
                sqlite: &sqlite,
                notif: &None,
                focus: &None,
                input_collector: &input_collector,
                event_tx: &event_tx,
            },
            0,
            false,
        )
        .await;

        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "Active→Active (call 2) must not emit"
        );
    }
}
