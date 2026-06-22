//! `run_gui_tick` — single-tick execution of the GUI activity pipeline.

use maekon_core::models::event::InputActivityEvent;
use maekon_core::models::focused_element::FocusedElementInfo;
use maekon_core::models::frame::OcrRegion;
use maekon_core::models::gui_activity::GuiActivitySummary;
use maekon_core::models::gui_interaction::{
    GuiElement, GuiElementType, GuiInteractionEvent, GuiInteractionType, InteractionType,
};
use maekon_vision::contour_classifier::feedback::{self, UncertainElement};

use chrono::Utc;

use super::state::GuiPipelineState;

/// Maximum uncertain elements buffered for LLM feedback.
const MAX_UNCERTAIN_QUEUE: usize = 20;
/// Confidence threshold below which elements are queued for LLM feedback.
const UNCERTAIN_THRESHOLD: f32 = 0.6;

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
    use maekon_vision::gui_detector::GuiElementDetector;

    let now = Utc::now();
    let mut result: Option<GuiActivitySummary> = state.pending_summaries.pop_front();

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
                for (from, to) in corrections.iter().rev() {
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
                    use maekon_vision::contour_classifier::features::extract_visual_features;
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
                bbox: maekon_core::models::frame::BoundingBox {
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
            record_summary(&mut result, &mut state.pending_summaries, summary);
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
                    bbox: maekon_core::models::frame::BoundingBox {
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
                record_summary(&mut result, &mut state.pending_summaries, summary);
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
                bbox: maekon_core::models::frame::BoundingBox {
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
            record_summary(&mut result, &mut state.pending_summaries, summary);
        }
    }

    result
}

fn record_summary(
    result: &mut Option<GuiActivitySummary>,
    pending: &mut std::collections::VecDeque<GuiActivitySummary>,
    summary: GuiActivitySummary,
) {
    if result.is_none() {
        *result = Some(summary);
    } else {
        pending.push_back(summary);
    }
}
