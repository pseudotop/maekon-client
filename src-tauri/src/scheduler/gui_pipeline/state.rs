//! `GuiPipelineState` — mutable state owned by the monitor loop.

use std::collections::{HashMap, VecDeque};

use maekon_analysis::gui_aggregator::GuiActivityAggregator;
use maekon_core::models::gui_activity::GuiActivitySummary;
use maekon_core::models::gui_interaction::GuiElementType;
use maekon_vision::contour_classifier::feedback::UncertainElement;
use maekon_vision::gui_detector::GuiElementDetector;

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
    /// Flushes produced after the first summary in a single tick.
    pub pending_summaries: VecDeque<GuiActivitySummary>,
}
