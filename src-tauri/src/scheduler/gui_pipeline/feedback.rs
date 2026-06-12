//! `process_gui_feedback` — LLM-based contour classifier feedback loop.

use std::sync::Arc;

use maekon_core::config::PiiFilterLevel;
use maekon_core::models::gui_interaction::GuiElementType;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::sanitized;
use maekon_vision::contour_classifier::feedback::{self, FeedbackRequest};

use super::state::GuiPipelineState;

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
    provider: &dyn maekon_core::ports::analysis_provider::AnalysisProvider,
    pii_sanitizer: Option<&Arc<dyn PiiSanitizer>>,
    pii_level: PiiFilterLevel,
) {
    let batch: Vec<_> = state
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
