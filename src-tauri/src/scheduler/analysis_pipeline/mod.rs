//! Extracted analysis pipeline helpers for the adaptive tiered-memory system.
//!
//! These functions were previously inlined in the monitor loop body inside
//! `loops.rs`. Extracting them keeps the loop orchestrator concise while the
//! heavy analysis logic lives here.
//!
//! ## Directory Module Structure (ADR-003)
//!
//! - `mod.rs` — main `run_analysis_tick()` orchestrator + re-exports
//! - `segment.rs` — segment lifecycle (open / close / restart / embedding)
//! - `regime.rs` — periodic regime detection + constrained clustering

mod regime;
mod segment;
#[cfg(test)]
mod tests;

use chrono::Utc;
use oneshim_core::models::app_registry::AppSubcategory;
use oneshim_core::models::event::InputActivityEvent;
use oneshim_core::models::focused_element::FocusedElementInfo;
use oneshim_core::models::gui_activity::GuiActivitySummary;
use oneshim_core::models::tiered_memory::TriggerInput;
use oneshim_core::models::work_session::AppCategory;
use oneshim_core::ports::storage::StorageService;
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::AdaptiveTriggerState;

/// Run a single tick of the adaptive tiered-memory pipeline.
///
/// This covers:
/// 1. TriggerInput classification
/// 2. Title-bar parsing & work-type classification
/// 3. Content tracking
/// 4. Regime classification & parameter cascade
/// 5. AdaptiveTrigger evaluation
/// 6. Calibration buffer flush
/// 7. Segment lifecycle (open / close / restart)
/// 8. Embedding pipeline (Phase 1 immediate + Phase 2 async spawn)
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_analysis_tick(
    ts: &mut AdaptiveTriggerState,
    app_name: &str,
    window_title: &str,
    prev_app: &Option<String>,
    app_changed: bool,
    input_snap: &InputActivityEvent,
    gui_summary: Option<&GuiActivitySummary>,
    focused_element: Option<&FocusedElementInfo>,
    storage: &Arc<dyn StorageService>,
) {
    let now = Utc::now();

    // 1. Classify event → TriggerInput
    let trigger_input = if app_changed {
        TriggerInput::AppSwitchNew {
            app_name: app_name.to_string(),
            prev_app: prev_app.clone().unwrap_or_default(),
            category: AppCategory::from_app_name(app_name),
        }
    } else {
        TriggerInput::AppPoll {
            app_name: app_name.to_string(),
        }
    };

    // 2. Parse title bar → content
    let parsed_content = ts.title_bar_parser.parse(app_name, window_title);

    // 3. Input activity snapshot (passed in by caller — shared with GUI pipeline)

    // 4. Classify work type
    let (work_type, engagement) = if let Some(ref content) = parsed_content {
        ts.work_type_classifier.classify(
            &input_snap.keyboard,
            &input_snap.mouse,
            &content.content_label,
            AppCategory::from_app_name(app_name),
        )
    } else {
        (
            oneshim_core::models::tiered_memory::WorkType::Unknown,
            oneshim_core::models::tiered_memory::EngagementMetrics::default(),
        )
    };

    // 4b. Refine work type using GUI signals (if available)
    let work_type = if let Some(gui) = gui_summary {
        ts.gui_work_type_refiner.refine(work_type, gui)
    } else {
        work_type
    };

    // 4c. Refine work type using accessibility role when focused element has
    //     a text-input role (AXTextArea, AXTextField). This helps distinguish
    //     e.g., terminal panel vs. editor panel when the app is an IDE.
    let work_type = if let Some(fe) = focused_element {
        match fe.role.as_str() {
            "AXTextArea" | "AXTextField" | "edit" | "document" => {
                // Element is a text input -- classification stands or can be
                // strengthened. No override needed; the subcategory rules from
                // classify_extended() already handle this.
                work_type
            }
            "AXStaticText" | "text" => {
                // Focused on static text (likely reading). If current type
                // is an active type, consider downgrading.
                match work_type {
                    oneshim_core::models::tiered_memory::WorkType::ActiveCoding
                    | oneshim_core::models::tiered_memory::WorkType::Writing
                    | oneshim_core::models::tiered_memory::WorkType::DocumentWriting => {
                        if engagement.keystrokes_per_min < 5.0 {
                            oneshim_core::models::tiered_memory::WorkType::Reading
                        } else {
                            work_type
                        }
                    }
                    _ => work_type,
                }
            }
            _ => work_type,
        }
    } else {
        work_type
    };

    // 4d. LLM refinement (async, optional — background prefetch + cached result)
    let work_type = if let Some(ref refiner) = ts.llm_work_type_refiner {
        let ocr_text: Option<String> = focused_element
            .and_then(|fe| fe.extracted_text.as_ref())
            .map(|s| s.to_string());
        refiner
            .refine(
                work_type,
                app_name,
                window_title,
                focused_element.map(|fe| fe.role.as_str()),
                ocr_text.as_deref(),
                engagement.keystrokes_per_min,
            )
            .await
            .unwrap_or(work_type)
    } else {
        work_type
    };

    // 4e. Enrich terminal commands with accessibility text
    let app_subcategory = ts.app_registry.classify(app_name).1;

    let terminal_command = focused_element
        .and_then(|fe| fe.extracted_text.as_deref())
        .and_then(|text| {
            if app_subcategory == AppSubcategory::Terminal {
                oneshim_analysis::terminal_detector::detect_terminal_command(text)
            } else {
                None
            }
        });

    if let Some(ref cmd_info) = terminal_command {
        debug!(
            command = %cmd_info.command,
            "Terminal command detected from accessibility text"
        );
    }

    // 4f. Extract document heading from accessibility text for document editors
    let doc_heading = focused_element
        .and_then(|fe| fe.extracted_text.as_deref())
        .and_then(|text| {
            if app_subcategory == AppSubcategory::DocumentEditor {
                oneshim_analysis::document_heading::extract_document_heading(text)
            } else {
                None
            }
        });

    if let Some(ref heading) = doc_heading {
        debug!(
            heading = %heading.heading,
            level = heading.level,
            "Document heading detected from accessibility text"
        );
    }

    // 5. Update content tracker
    // Enrich content_label with terminal command or document heading when
    // detected, so the information propagates through to the segment
    // summarizer and eventually the LLM context (via ContentSummaryEntry).
    if let Some(ref content) = parsed_content {
        let enriched_label = if let Some(ref cmd_info) = terminal_command {
            // Append the detected terminal command to the content label
            // so the LLM knows what command the user is running.
            format!("{} [$ {}]", content.content_label, cmd_info.command_line)
        } else if let Some(ref heading) = doc_heading {
            // Append the document heading to help the LLM understand
            // which section the user is working on.
            format!("{} — {}", content.content_label, heading.heading)
        } else {
            content.content_label.clone()
        };

        ts.content_tracker
            .update(oneshim_analysis::content_tracker::ContentUpdateInput {
                content_label: enriched_label,
                content_type: content.content_type,
                work_type,
                engagement: engagement.clone(),
                confidence: content.confidence,
                timestamp: now,
                gui_summary: gui_summary.cloned(),
            });
    }

    // 5b. Regime classification → parameter cascade
    //
    // Clone the Regime out of the classifier lock scope so `current_regime`
    // can be passed to `param_resolver.resolve` below without holding the
    // classifier lock across `param_resolver` — smaller critical section,
    // no lock-ordering concern.
    let app_category = AppCategory::from_app_name(app_name);
    let current_regime_owned: Option<oneshim_core::models::tiered_memory::Regime> = {
        let cls = ts.regime_classifier.lock();
        cls.classify(&oneshim_core::models::tiered_memory::RegimeFeatures {
            category_coding: if app_category == AppCategory::Development {
                1.0
            } else {
                0.0
            },
            category_communication: if app_category == AppCategory::Communication {
                1.0
            } else {
                0.0
            },
            category_browser: if app_category == AppCategory::Browser {
                1.0
            } else {
                0.0
            },
            avg_event_rate: ts.trigger.current_density_signal(),
            avg_importance: ts.trigger.current_importance_signal(),
            context_activity_signal: ts.trigger.current_context_signal(),
            communication_ratio: if app_category == AppCategory::Communication {
                1.0
            } else {
                0.0
            },
        })
        .cloned()
    };
    let current_regime = current_regime_owned.as_ref();

    // Detect regime transition
    let new_regime_id = current_regime.map(|r| r.regime_id.clone());
    if new_regime_id != ts.current_regime_id {
        let from_label = ts.current_regime_id.clone();
        if let Some(ref to_id) = new_regime_id {
            info!(
                from = ?from_label,
                to = %to_id,
                "regime transition detected"
            );
        }
        ts.current_regime_id = new_regime_id.clone();
    }

    // Mark regime as seen
    if let Some(ref regime_id) = new_regime_id {
        ts.regime_manager.lock().mark_seen(regime_id, now);
    }

    // Resolve params via CSS cascade
    let resolved = ts
        .param_resolver
        .resolve(current_regime, &app_category, app_name);
    ts.params = resolved;

    // 5c. Auto-tuner: update EMA stats per tick
    let density = ts.trigger.current_density_signal();
    let importance = ts.trigger.current_importance_signal();
    let cat_str = format!("{:?}", app_category).to_lowercase();
    ts.ema_tracker
        .update(&cat_str, app_name, density, importance);

    ts.auto_tune_tick_count += 1;

    // Periodically (every 100 ticks): generate overrides → ParamResolver
    if ts.auto_tune_tick_count % 100 == 0 {
        let overrides = ts.ema_tracker.generate_overrides();
        for (cat_key, params) in &overrides {
            let category = AppCategory::from_category_str(cat_key);
            ts.param_resolver
                .set_category_override(category, params.clone());
        }
        if !overrides.is_empty() {
            debug!(count = overrides.len(), "auto-tune overrides applied");
        }

        // Check drift on importance signal
        if ts.drift_detector.observe(importance) {
            info!("drift detected — flagging for re-clustering");
            ts.recluster_requested
                .store(true, std::sync::atomic::Ordering::Relaxed);
            ts.last_drift_detected
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    // 6. Feed to AdaptiveTrigger
    let (decision, cal_entry) = ts.trigger.process_event(&trigger_input, now, &ts.params);

    // 7. Buffer calibration entry
    if let Some(batch) = ts.calibration_buffer.push(cal_entry) {
        if let Err(e) = ts.calibration_writer.log_batch(&batch) {
            warn!("calibration log failure: {e}");
        }
    }

    // 8. Handle segment lifecycle
    segment::handle_segment_lifecycle(ts, decision, trigger_input, now, storage).await;

    // --- Periodic regime detection (daily) ---
    regime::run_periodic_regime_detection(ts, now).await;
}
