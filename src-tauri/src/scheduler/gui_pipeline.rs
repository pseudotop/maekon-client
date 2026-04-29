//! GUI Activity Intelligence pipeline for the scheduler.
//!
//! Integrates `GuiElementDetector` and `GuiActivityAggregator` into a single
//! `run_gui_tick()` function that follows the `analysis_pipeline::run_analysis_tick()`
//! pattern. `GuiWorkTypeRefiner` is called from the analysis pipeline, not here
//! (see `GuiPipelineState` doc comment for rationale).
//!
//! Called from the monitor loop after `run_analysis_tick()`. The returned
//! `GuiActivitySummary` is fed into `ContentTracker` on the next tick.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use oneshim_analysis::gui_aggregator::GuiActivityAggregator;
use oneshim_core::config::PiiFilterLevel;
use oneshim_core::models::event::InputActivityEvent;
use oneshim_core::models::focused_element::FocusedElementInfo;
use oneshim_core::models::frame::OcrRegion;
use oneshim_core::models::gui_activity::GuiActivitySummary;
use oneshim_core::models::gui_interaction::{
    GuiElement, GuiElementType, GuiInteractionEvent, GuiInteractionType, InteractionType,
};
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use oneshim_core::sanitized;
use oneshim_vision::contour_classifier::feedback::{self, FeedbackRequest, UncertainElement};
use oneshim_vision::gui_detector::GuiElementDetector;

use chrono::Utc;

/// Maximum uncertain elements buffered for LLM feedback.
const MAX_UNCERTAIN_QUEUE: usize = 20;
/// Confidence threshold below which elements are queued for LLM feedback.
const UNCERTAIN_THRESHOLD: f32 = 0.6;

/// D5 iter-16: resolve the `PiiFilterLevel` for GUI-feedback tracing from an
/// optional `ConfigManager`. Exposed at crate root so the monitor loop can
/// call it inline without growing its LOC budget.
pub(crate) fn gui_feedback_pii_level(
    config_manager: &Option<oneshim_core::config_manager::ConfigManager>,
) -> PiiFilterLevel {
    config_manager
        .as_ref()
        .map(|cm| cm.get().privacy.pii_filter_level)
        .unwrap_or_default()
}

/// D5 iter-16: build the `PiiSanitizer` used at the GUI-feedback tracing
/// boundary. Exposed at crate root so the monitor loop can call it inline
/// without re-stating the multi-line `Option<Arc<dyn ...>>` type — the
/// `monitor::spawn_monitor_loop` body is hard-capped at 500 lines by the
/// lefthook guard, so every line counts.
pub(crate) fn gui_feedback_pii_sanitizer() -> Option<Arc<dyn PiiSanitizer>> {
    Some(Arc::new(oneshim_vision::privacy::VisionPiiSanitizer))
}

/// Mutable state for the GUI pipeline, owned by the monitor loop.
///
/// Note: `GuiWorkTypeRefiner` is intentionally NOT included here. The refiner
/// requires an initial `WorkType` from the analysis pipeline, which runs
/// separately. `GuiWorkTypeRefiner::refine()` is called from the analysis
/// pipeline after it receives the `GuiActivitySummary` produced by this
/// pipeline, not from the GUI pipeline itself.
pub(crate) struct GuiPipelineState {
    pub detector: GuiElementDetector,
    pub aggregator: GuiActivityAggregator,
    /// Uncertain elements queued for LLM feedback.
    pub uncertain_queue: VecDeque<UncertainElement>,
    /// Ticks since last feedback batch.
    pub feedback_tick_counter: u32,
    /// Cached LLM corrections per app: app_name → [(from_type, to_type)].
    pub app_type_cache: HashMap<String, Vec<(GuiElementType, GuiElementType)>>,
}

/// Run a single tick of the GUI activity intelligence pipeline.
///
/// Steps:
/// 1. Correlate mouse clicks with OCR regions via `GuiElementDetector`
/// 2. Build `GuiInteractionEvent`s
/// 3. Push events into `GuiActivityAggregator`
/// 4. If aggregator flushes, return the summary
///
/// The caller (monitor loop) feeds the returned summary into
/// `ContentTracker::update()`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_gui_tick(
    state: &mut GuiPipelineState,
    ocr_regions: &[OcrRegion],
    input_snap: &InputActivityEvent,
    recent_shortcuts: &[String],
    app_name: &str,
    window_title: &str,
    content_label: &str,
    focused_element: Option<&FocusedElementInfo>,
    frame_rgba: Option<&[u8]>,
    frame_width: u32,
    frame_height: u32,
) -> Option<GuiActivitySummary> {
    let now = Utc::now();
    let mut result: Option<GuiActivitySummary> = None;

    // 1. Correlate mouse clicks with OCR regions
    if input_snap.mouse.click_count > 0 {
        // Use the last known mouse position from the input snapshot.
        // InputActivityEvent provides aggregate counts; we generate one
        // representative event per tick when clicks are detected.
        let (click_x, click_y) = input_snap
            .mouse
            .last_position
            .map(|(x, y)| (x as u32, y as u32))
            .unwrap_or((0, 0));

        let mut element =
            state
                .detector
                .correlate_click_with_app(click_x, click_y, ocr_regions, app_name);

        // ML classifier upgrade: re-classify the matched element for higher accuracy
        if element.is_some() && state.detector.ml_classifier().is_some() && frame_rgba.is_some() {
            // Find the clicked OCR region directly (avoids fragile bbox reverse-lookup)
            let region_for_ml = ocr_regions
                .iter()
                .filter(|r| r.bbox.contains_point(click_x, click_y))
                .min_by_key(|r| r.bbox.area());

            if let Some(region) = region_for_ml {
                let ml_elem = state
                    .detector
                    .build_gui_element_with_frame(region, frame_rgba, frame_width, frame_height)
                    .await;
                element = Some(ml_elem);
            }
        }

        // Apply cached LLM corrections for this app
        if let Some(ref mut elem) = element {
            if let Some(corrections) = state.app_type_cache.get(app_name) {
                for (from, to) in corrections {
                    if elem.element_type == *from {
                        elem.element_type = to.clone();
                        elem.type_confidence = 1.0; // Prevent re-queuing corrected elements
                        break;
                    }
                }
            }

            // Queue uncertain elements for LLM feedback with visual features
            if elem.type_confidence < UNCERTAIN_THRESHOLD
                && state.uncertain_queue.len() < MAX_UNCERTAIN_QUEUE
            {
                // Extract visual features from crop if frame data available
                let features = if let Some(frame) = frame_rgba {
                    use oneshim_vision::contour_classifier::features::extract_visual_features;
                    if let Some(crop) = GuiElementDetector::crop_region_rgba(
                        frame,
                        frame_width,
                        frame_height,
                        &elem.bbox,
                    ) {
                        let vf = extract_visual_features(&crop, elem.bbox.width, elem.bbox.height);
                        feedback::FeatureSummary::from(&vf)
                    } else {
                        feedback::FeatureSummary::from_aspect_ratio(
                            elem.bbox.width as f32 / elem.bbox.height.max(1) as f32,
                        )
                    }
                } else {
                    feedback::FeatureSummary::from_aspect_ratio(
                        elem.bbox.width as f32 / elem.bbox.height.max(1) as f32,
                    )
                };
                state.uncertain_queue.push_back(UncertainElement {
                    app_name: app_name.to_string(),
                    text: elem.text.clone(),
                    current_type: format!("{:?}", elem.element_type),
                    confidence: elem.type_confidence,
                    features,
                });
            }
        }

        let gui_element = element.unwrap_or_else(|| {
            // If accessibility provides a focused element label, use it as
            // a better fallback than a completely empty element.
            let (text, element_type) = focused_element
                .and_then(|fe| {
                    fe.label.as_ref().map(|label| {
                        let etype = match fe.role.as_str() {
                            "AXButton" => GuiElementType::Button,
                            "AXTextField" | "AXTextArea" | "edit" => GuiElementType::TextInput,
                            "AXMenuItem" | "AXMenu" => GuiElementType::MenuItem,
                            _ => GuiElementType::Unknown,
                        };
                        (label.clone(), etype)
                    })
                })
                .unwrap_or((String::new(), GuiElementType::Unknown));

            GuiElement {
                text,
                bbox: oneshim_core::models::frame::BoundingBox {
                    x: click_x,
                    y: click_y,
                    width: 1,
                    height: 1,
                },
                element_type,
                confidence: if focused_element.is_some() { 0.6 } else { 0.0 },
                type_confidence: 1.0,
            }
        });

        let interaction_event = GuiInteractionEvent {
            timestamp: now,
            element: gui_element,
            interaction_type: GuiInteractionType::Click,
            app_name: app_name.to_string(),
            window_title: Some(window_title.to_string()),
            screen_position: Some((click_x, click_y)),
            interaction: None,
        };

        if let Some(summary) = state.aggregator.push(interaction_event, content_label) {
            result = Some(summary);
        }
    }

    // 2. Handle keyboard shortcuts (if detected in input snapshot)
    //    Iterate over ALL shortcuts that occurred this tick, not just the first.
    if input_snap.keyboard.shortcut_count > 0 {
        for shortcut_keys in recent_shortcuts {
            let shortcut_event = GuiInteractionEvent {
                timestamp: now,
                element: GuiElement {
                    text: String::new(),
                    bbox: oneshim_core::models::frame::BoundingBox {
                        x: 0,
                        y: 0,
                        width: 0,
                        height: 0,
                    },
                    element_type: GuiElementType::Unknown,
                    confidence: 0.0,
                    type_confidence: 1.0,
                },
                interaction_type: GuiInteractionType::Type,
                app_name: app_name.to_string(),
                window_title: Some(window_title.to_string()),
                screen_position: None,
                interaction: Some(InteractionType::KeyboardShortcut {
                    keys: shortcut_keys.clone(),
                }),
            };

            if let Some(summary) = state.aggregator.push(shortcut_event, content_label) {
                result = Some(summary);
            }
        }
    }

    // 3. Handle text entry — detect remaining keystrokes after subtracting
    //    shortcut keystrokes so text entry is not suppressed when shortcuts
    //    are also present in the same tick.
    let text_keystrokes = input_snap
        .keyboard
        .total_keystrokes
        .saturating_sub(input_snap.keyboard.shortcut_count);
    if text_keystrokes > 0 {
        let text_event = GuiInteractionEvent {
            timestamp: now,
            element: GuiElement {
                text: String::new(),
                bbox: oneshim_core::models::frame::BoundingBox {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                element_type: GuiElementType::TextInput,
                confidence: 0.5,
                type_confidence: 1.0,
            },
            interaction_type: GuiInteractionType::Type,
            app_name: app_name.to_string(),
            window_title: Some(window_title.to_string()),
            screen_position: None,
            interaction: Some(InteractionType::TextEntry {
                char_count: text_keystrokes,
                duration_ms: 0,
            }),
        };

        if let Some(summary) = state.aggregator.push(text_event, content_label) {
            result = Some(summary);
        }
    }

    result
}

/// Process queued uncertain elements by sending them to the LLM for feedback.
///
/// Called periodically from the monitor loop when `feedback_tick_counter`
/// reaches `FEEDBACK_INTERVAL_TICKS` and the uncertain queue is non-empty.
///
/// D5 iter-16: `pii_sanitizer` + `pii_level` scrub LLM-error Display output
/// at the tracing boundary (the LLM may echo prompt content — window titles,
/// app names, `UncertainElement` metadata — back in its error body).
pub(crate) async fn process_gui_feedback(
    state: &mut GuiPipelineState,
    provider: &dyn oneshim_core::ports::analysis_provider::AnalysisProvider,
    pii_sanitizer: Option<&Arc<dyn PiiSanitizer>>,
    pii_level: PiiFilterLevel,
) {
    let batch: Vec<UncertainElement> = state
        .uncertain_queue
        .drain(..state.uncertain_queue.len().min(5))
        .collect();

    if batch.is_empty() {
        return;
    }

    let request = FeedbackRequest {
        uncertain_elements: batch.clone(),
    };
    let request_json = match serde_json::to_string(&request) {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!("GUI feedback request serialization failed: {e}");
            return;
        }
    };

    match provider
        .summarize_text(&request_json, feedback::CONTOUR_FEEDBACK_PROMPT)
        .await
    {
        Ok(response_str) => {
            match feedback::parse_feedback_response(&response_str) {
                Ok(response) => {
                    let mut applied = 0;
                    for correction in &response.corrections {
                        if correction.index >= batch.len() {
                            continue;
                        }
                        let Some(correct_type) =
                            feedback::validate_element_type(&correction.correct_type)
                        else {
                            tracing::debug!(
                                "Ignoring invalid LLM type: {}",
                                correction.correct_type
                            );
                            continue;
                        };
                        if correction.confidence < 0.5 {
                            continue;
                        }

                        let elem = &batch[correction.index];
                        let from_type = feedback::validate_element_type(&elem.current_type)
                            .unwrap_or(GuiElementType::Unknown);

                        // Cache the correction for this app (bounded: 24 per app, 256 apps)
                        if state.app_type_cache.len() < 256
                            || state.app_type_cache.contains_key(&elem.app_name)
                        {
                            let entries = state
                                .app_type_cache
                                .entry(elem.app_name.clone())
                                .or_default();
                            if entries.len() >= 24 {
                                entries.remove(0);
                            }
                            entries.push((from_type, correct_type));
                        }

                        applied += 1;
                    }
                    if applied > 0 {
                        tracing::info!(
                            applied,
                            apps = state.app_type_cache.len(),
                            "GUI feedback corrections applied"
                        );
                    }
                }
                Err(e) => {
                    tracing::debug!("GUI feedback response parse failed: {e}");
                }
            }
        }
        Err(e) => {
            // D5 iter-16: LLM error body can echo prompt content (window
            // titles, app names, UncertainElement metadata). Route through
            // `SanitizedDisplay` when a sanitizer is attached.
            match pii_sanitizer {
                Some(san) => tracing::debug!(
                    err.code = %e.code(),
                    "GUI feedback LLM call failed: {}",
                    sanitized(&e, san.as_ref(), pii_level),
                ),
                None => tracing::debug!(err.code = %e.code(), "GUI feedback LLM call failed: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::config::{GuiIntelligenceConfig, PiiFilterLevel};
    use oneshim_core::models::event::{KeyboardActivity, MouseActivity};
    use oneshim_core::models::frame::BoundingBox;

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

    use async_trait::async_trait;
    use oneshim_core::error::CoreError;
    use oneshim_core::ports::gui_element_classifier::GuiElementClassifier;
    use std::sync::Arc;

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
