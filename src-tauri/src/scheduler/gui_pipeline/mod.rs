//! GUI Activity Intelligence pipeline for the scheduler.
//!
//! Integrates `GuiElementDetector` and `GuiActivityAggregator` into a single
//! `run_gui_tick()` function that follows the `analysis_pipeline::run_analysis_tick()`
//! pattern. `GuiWorkTypeRefiner` is called from the analysis pipeline, not here
//! (see `GuiPipelineState` doc comment for rationale).
//!
//! Called from the monitor loop after `run_analysis_tick()`. The returned
//! `GuiActivitySummary` is fed into `ContentTracker` on the next tick.

mod config_helpers;
mod feedback;
mod state;
mod tick;

pub(crate) use config_helpers::{gui_feedback_pii_level, gui_feedback_pii_sanitizer};
pub(crate) use feedback::process_gui_feedback;
pub(crate) use state::GuiPipelineState;
pub(crate) use tick::run_gui_tick;

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::Utc;
    use maekon_analysis::gui_aggregator::GuiActivityAggregator;
    use maekon_core::config::{GuiIntelligenceConfig, PiiFilterLevel};
    use maekon_core::error::CoreError;
    use maekon_core::models::event::{KeyboardActivity, MouseActivity};
    use maekon_core::models::frame::BoundingBox;
    use maekon_core::models::frame::OcrRegion;
    use maekon_core::models::gui_interaction::GuiElementType;
    use maekon_core::ports::gui_element_classifier::GuiElementClassifier;
    use maekon_vision::gui_detector::GuiElementDetector;

    use super::state::GuiPipelineState;
    use super::tick::run_gui_tick;

    use maekon_core::models::event::InputActivityEvent;

    /// Helper: build a `GuiPipelineState` with 1920x1080 detector and a
    /// short aggregation window for test-friendly flushing.
    fn make_state(window_secs: u64, max_events: usize) -> GuiPipelineState {
        let config = GuiIntelligenceConfig {
            enabled: true,
            aggregation_window_secs: window_secs,
            max_events_per_segment: max_events,
            proximity_threshold_px: 40,
            ml_model_path: String::new(),
        };
        GuiPipelineState {
            detector: GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off),
            aggregator: GuiActivityAggregator::new(&config),
            uncertain_queue: VecDeque::new(),
            feedback_tick_counter: 0,
            app_type_cache: HashMap::new(),
        }
    }

    /// Helper: build an `InputActivityEvent` with the given click/keyboard params.
    fn make_input(
        click_count: u32,
        last_pos: Option<(f32, f32)>,
        total_keystrokes: u32,
        shortcut_count: u32,
    ) -> InputActivityEvent {
        InputActivityEvent {
            timestamp: Utc::now(),
            period_secs: 3,
            mouse: MouseActivity {
                click_count,
                move_distance: 0.0,
                scroll_count: 0,
                last_position: last_pos,
                double_click_count: 0,
                right_click_count: 0,
            },
            keyboard: KeyboardActivity {
                keystrokes_per_min: 0,
                total_keystrokes,
                typing_bursts: 0,
                shortcut_count,
                correction_count: 0,
            },
            app_name: "VS Code".to_string(),
            keystroke_profile: None,
        }
    }

    fn make_ocr_region(text: &str, x: u32, y: u32, w: u32, h: u32) -> OcrRegion {
        OcrRegion {
            text: text.to_string(),
            bbox: BoundingBox {
                x,
                y,
                width: w,
                height: h,
            },
            confidence: 0.9,
        }
    }

    #[tokio::test]
    async fn click_with_ocr_produces_correlated_event() {
        let mut state = make_state(60, 100);

        // Place an OCR region ("Save" button) near the click position
        let regions = vec![make_ocr_region("Save", 490, 290, 60, 30)];
        let input = make_input(1, Some((500.0, 300.0)), 0, 0);

        // First tick: event goes into aggregator buffer (no flush yet)
        let result = run_gui_tick(
            &mut state,
            &regions,
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            None,
            0,
            0,
        )
        .await;

        // No flush yet (only 1 event, window not expired)
        assert!(result.is_none());

        // Force flush via content label change
        let input2 = make_input(1, Some((500.0, 300.0)), 0, 0);
        let result = run_gui_tick(
            &mut state,
            &regions,
            &input2,
            &[],
            "VS Code",
            "lib.rs",
            "lib.rs", // different content_label triggers flush
            None,
            None,
            0,
            0,
        )
        .await;

        let summary = result.expect("content label change should flush");
        assert_eq!(summary.content_label, "main.rs");
        assert!(summary.button_clicks > 0 || summary.save_count > 0);
    }

    #[tokio::test]
    async fn click_with_empty_ocr_produces_unknown_element() {
        // max_events=1 so the 2nd event triggers a flush of the 1st window
        let mut state = make_state(60, 1);

        let input = make_input(1, Some((500.0, 300.0)), 0, 0);

        // Push first event (fills the 1-event window)
        run_gui_tick(
            &mut state,
            &[],
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            None,
            0,
            0,
        )
        .await;

        // Push second event — triggers flush due to max_events=1
        let result = run_gui_tick(
            &mut state,
            &[],
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            None,
            0,
            0,
        )
        .await;

        let summary = result.expect("max_events should trigger flush");
        // No OCR regions → the click lands on an Unknown element
        assert_eq!(summary.unmatched_click_count, 1);
    }

    #[tokio::test]
    async fn keyboard_only_produces_text_entry() {
        let mut state = make_state(60, 1);

        // No clicks, just keystrokes
        let input = make_input(0, None, 20, 0);

        run_gui_tick(
            &mut state,
            &[],
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            None,
            0,
            0,
        )
        .await;

        // Second event to flush
        let result = run_gui_tick(
            &mut state,
            &[],
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            None,
            0,
            0,
        )
        .await;

        let summary = result.expect("should flush via max_events");
        assert!(summary.text_entries > 0);
    }

    #[tokio::test]
    async fn shortcuts_iterate_all() {
        let mut state = make_state(60, 100);

        let input = make_input(0, None, 3, 3);
        let shortcuts = vec![
            "Cmd+S".to_string(),
            "Cmd+F".to_string(),
            "Cmd+Z".to_string(),
        ];

        // Push events
        run_gui_tick(
            &mut state,
            &[],
            &input,
            &shortcuts,
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            None,
            0,
            0,
        )
        .await;

        // Flush via content change
        let input2 = make_input(1, Some((100.0, 100.0)), 0, 0);
        let result = run_gui_tick(
            &mut state,
            &[],
            &input2,
            &[],
            "VS Code",
            "lib.rs",
            "lib.rs",
            None,
            None,
            0,
            0,
        )
        .await;

        let summary = result.expect("content change should flush");
        // All 3 shortcuts were fed as events
        assert_eq!(summary.save_count, 1); // Cmd+S
        assert_eq!(summary.search_count, 1); // Cmd+F
        assert_eq!(summary.undo_redo_count, 1); // Cmd+Z
    }

    #[tokio::test]
    async fn mixed_clicks_and_typing() {
        let mut state = make_state(60, 100);

        // Click + typing in same tick
        let input = make_input(1, Some((500.0, 300.0)), 15, 0);
        let regions = vec![make_ocr_region("Search", 490, 290, 80, 30)];

        run_gui_tick(
            &mut state,
            &regions,
            &input,
            &[],
            "Chrome",
            "Google",
            "search",
            None,
            None,
            0,
            0,
        )
        .await;

        // Flush via content change
        let input2 = make_input(1, Some((100.0, 100.0)), 0, 0);
        let result = run_gui_tick(
            &mut state,
            &[],
            &input2,
            &[],
            "Chrome",
            "Results",
            "results",
            None,
            None,
            0,
            0,
        )
        .await;

        let summary = result.expect("should flush on content change");
        assert!(summary.button_clicks > 0 || summary.search_count > 0);
        assert!(summary.text_entries > 0);
    }

    #[tokio::test]
    async fn no_input_produces_nothing() {
        let mut state = make_state(60, 100);

        // Zero clicks, zero keystrokes
        let input = make_input(0, None, 0, 0);

        let result = run_gui_tick(
            &mut state,
            &[],
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            None,
            0,
            0,
        )
        .await;

        assert!(result.is_none());
        // Even flushing should return None since no events were pushed
        let flushed = state.aggregator.flush();
        assert!(flushed.is_none());
    }

    // --- ML classifier integration tests ---

    /// Mock ML classifier that always returns Button with configurable confidence.
    struct MockClassifier {
        confidence: f32,
    }

    #[async_trait]
    impl GuiElementClassifier for MockClassifier {
        async fn classify_crop(
            &self,
            _crop_rgba: &[u8],
            _width: u32,
            _height: u32,
        ) -> Result<Option<(GuiElementType, f32)>, CoreError> {
            if self.confidence > 0.0 {
                Ok(Some((GuiElementType::Button, self.confidence)))
            } else {
                Ok(None)
            }
        }

        fn is_ready(&self) -> bool {
            true
        }
    }

    fn make_state_with_ml(
        window_secs: u64,
        max_events: usize,
        confidence: f32,
    ) -> GuiPipelineState {
        let config = GuiIntelligenceConfig {
            enabled: true,
            aggregation_window_secs: window_secs,
            max_events_per_segment: max_events,
            proximity_threshold_px: 40,
            ml_model_path: String::new(),
        };
        let classifier: Arc<dyn GuiElementClassifier> = Arc::new(MockClassifier { confidence });
        GuiPipelineState {
            detector: GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off)
                .with_ml_classifier(classifier),
            aggregator: GuiActivityAggregator::new(&config),
            uncertain_queue: VecDeque::new(),
            feedback_tick_counter: 0,
            app_type_cache: HashMap::new(),
        }
    }

    /// Make a minimal RGBA frame buffer (all gray pixels).
    fn make_frame_rgba(width: u32, height: u32) -> Vec<u8> {
        vec![128u8; (width * height * 4) as usize]
    }

    #[tokio::test]
    async fn ml_classifier_overrides_heuristic_on_high_confidence() {
        // ML returns Button with 0.95 confidence
        let mut state = make_state_with_ml(60, 1, 0.95);

        // OCR region: "Ln 42, Col 10" at bottom of screen → heuristic = StatusBar
        let regions = vec![make_ocr_region("Ln 42, Col 10", 0, 1050, 200, 20)];
        let frame = make_frame_rgba(1920, 1080);
        let input = make_input(1, Some((100.0, 1060.0)), 0, 0);

        // First tick (buffer)
        run_gui_tick(
            &mut state,
            &regions,
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            Some(&frame),
            1920,
            1080,
        )
        .await;

        // Flush via max_events
        let result = run_gui_tick(
            &mut state,
            &regions,
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            Some(&frame),
            1920,
            1080,
        )
        .await;

        let summary = result.expect("should flush");
        // ML classified as Button (overriding StatusBar heuristic)
        assert!(summary.button_clicks > 0, "ML should override to Button");
    }

    #[tokio::test]
    async fn ml_classifier_fallback_when_no_frame_data() {
        let mut state = make_state_with_ml(60, 1, 0.95);

        // StatusBar region, no frame data → heuristic should win
        let regions = vec![make_ocr_region("Ln 42, Col 10", 0, 1050, 200, 20)];
        let input = make_input(1, Some((100.0, 1060.0)), 0, 0);

        run_gui_tick(
            &mut state,
            &regions,
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            None,
            0,
            0, // No frame data
        )
        .await;

        let result = run_gui_tick(
            &mut state,
            &regions,
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            None,
            0,
            0,
        )
        .await;

        let summary = result.expect("should flush");
        // Without frame data, heuristic StatusBar classification should be used
        assert_eq!(summary.button_clicks, 0, "no ML without frame data");
    }

    #[tokio::test]
    async fn ml_classifier_low_confidence_still_produces_events() {
        // ML returns 0.5 confidence (below 0.7 threshold in build_gui_element_with_frame)
        // The pipeline should still work — heuristic is used as fallback
        let mut state = make_state_with_ml(60, 1, 0.5);

        let regions = vec![make_ocr_region("Save", 490, 490, 60, 30)];
        let frame = make_frame_rgba(1920, 1080);
        let input = make_input(1, Some((500.0, 500.0)), 0, 0);

        run_gui_tick(
            &mut state,
            &regions,
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            Some(&frame),
            1920,
            1080,
        )
        .await;

        let result = run_gui_tick(
            &mut state,
            &regions,
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            Some(&frame),
            1920,
            1080,
        )
        .await;

        let summary = result.expect("should flush even with low ML confidence");
        // Heuristic classifies "Save" as Button — event should be recorded
        assert!(summary.button_clicks > 0 || summary.save_count > 0);
    }

    #[tokio::test]
    async fn no_ml_classifier_preserves_existing_behavior() {
        // Standard state without ML classifier
        let mut state = make_state(60, 1);
        let frame = make_frame_rgba(1920, 1080);

        let regions = vec![make_ocr_region("Save", 490, 490, 60, 30)];
        let input = make_input(1, Some((500.0, 500.0)), 0, 0);

        run_gui_tick(
            &mut state,
            &regions,
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            Some(&frame),
            1920,
            1080, // Frame provided but no classifier
        )
        .await;

        let result = run_gui_tick(
            &mut state,
            &regions,
            &input,
            &[],
            "VS Code",
            "main.rs",
            "main.rs",
            None,
            Some(&frame),
            1920,
            1080,
        )
        .await;

        let summary = result.expect("should flush");
        assert!(summary.button_clicks > 0 || summary.save_count > 0);
    }
}
