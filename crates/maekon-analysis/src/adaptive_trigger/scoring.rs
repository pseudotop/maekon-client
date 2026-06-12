use maekon_core::models::tiered_memory::ResolvedParams;
use maekon_core::models::tiered_memory::TriggerInput;
use maekon_core::models::work_session::AppCategory;

/// Extract app name and category from a TriggerInput.
pub(super) fn extract_app_info(input: &TriggerInput) -> (String, AppCategory) {
    match input {
        TriggerInput::AppSwitchNew {
            app_name, category, ..
        } => (app_name.clone(), *category),
        TriggerInput::AppPoll { app_name } => {
            (app_name.clone(), AppCategory::from_app_name(app_name))
        }
        TriggerInput::WindowTitleChange { app_name, .. } => {
            (app_name.clone(), AppCategory::from_app_name(app_name))
        }
        TriggerInput::IdleTransition { .. } => ("system".to_string(), AppCategory::System),
        TriggerInput::OcrUpdate { .. } => ("ocr".to_string(), AppCategory::Other),
        TriggerInput::InputActivity => ("input".to_string(), AppCategory::Other),
        TriggerInput::ProcessSnapshot => ("process".to_string(), AppCategory::System),
        TriggerInput::SystemMetric => ("system".to_string(), AppCategory::System),
        TriggerInput::ClipboardChange => ("clipboard".to_string(), AppCategory::Other),
        TriggerInput::FileAccess => ("file".to_string(), AppCategory::Other),
        TriggerInput::WorkTypeChange { .. } => ("work_type".to_string(), AppCategory::Other),
    }
}

/// Return a stable string label for the TriggerInput variant.
pub(super) fn input_type_str(input: &TriggerInput) -> &'static str {
    match input {
        TriggerInput::AppSwitchNew { .. } => "APP_SWITCH_NEW",
        TriggerInput::AppPoll { .. } => "APP_POLL",
        TriggerInput::WindowTitleChange { .. } => "WINDOW_TITLE_CHANGE",
        TriggerInput::IdleTransition { .. } => "IDLE_TRANSITION",
        TriggerInput::OcrUpdate { .. } => "OCR_UPDATE",
        TriggerInput::InputActivity => "INPUT_ACTIVITY",
        TriggerInput::ProcessSnapshot => "PROCESS_SNAPSHOT",
        TriggerInput::SystemMetric => "SYSTEM_METRIC",
        TriggerInput::ClipboardChange => "CLIPBOARD_CHANGE",
        TriggerInput::FileAccess => "FILE_ACCESS",
        TriggerInput::WorkTypeChange { .. } => "WORK_TYPE_CHANGE",
    }
}

/// Score the raw importance of a single event, applying per-app overrides.
pub(super) fn score_importance(input: &TriggerInput, params: &ResolvedParams) -> f32 {
    let (app_name, _) = extract_app_info(input);

    // Check per-app override first
    if let Some(&override_score) = params.importance_overrides.get(&app_name) {
        return override_score.clamp(0.0, 1.0);
    }

    // Base importance by event type
    let base = match input {
        TriggerInput::AppSwitchNew { .. } => 0.8,
        TriggerInput::WindowTitleChange { .. } => 0.6,
        TriggerInput::OcrUpdate { diff_ratio, .. } => 0.4 + diff_ratio.clamp(0.0, 1.0) * 0.4,
        TriggerInput::IdleTransition { to_idle } => {
            if *to_idle {
                0.9
            } else {
                0.7
            }
        }
        TriggerInput::WorkTypeChange { .. } => 0.85,
        TriggerInput::ClipboardChange => 0.5,
        TriggerInput::FileAccess => 0.55,
        TriggerInput::InputActivity => 0.3,
        TriggerInput::AppPoll { .. } => 0.15,
        TriggerInput::ProcessSnapshot => 0.1,
        TriggerInput::SystemMetric => 0.05,
    };

    base.clamp(0.0, 1.0)
}
