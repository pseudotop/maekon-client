//! Helpers for resolving PII-related config at the GUI-feedback tracing boundary.

use std::sync::Arc;

use maekon_core::config::PiiFilterLevel;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;

/// D5 iter-16: resolve the `PiiFilterLevel` for GUI-feedback tracing from an
/// optional `ConfigManager`. Exposed at crate root so the monitor loop can
/// call it inline without growing its LOC budget.
pub(crate) fn gui_feedback_pii_level(
    config_manager: &Option<maekon_core::config_manager::ConfigManager>,
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
    Some(Arc::new(maekon_vision::privacy::VisionPiiSanitizer))
}
