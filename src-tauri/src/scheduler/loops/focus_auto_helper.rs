use oneshim_core::config::FocusAutoConfig;
use tracing::info;

use crate::focus_auto::FocusAutoEvaluator;
use crate::focus_mode::FocusModeState;
use crate::magic_overlay::MagicOverlayHandle;

/// Evaluate focus auto-switch rules and activate if triggered.
///
/// Called from the monitor loop after context collection provides the current
/// app name. Extracted into a helper to keep monitor.rs under 500 lines.
pub(super) fn evaluate_focus_auto(
    config: &FocusAutoConfig,
    focus_mode: &FocusModeState,
    current_app: &str,
    overlay: Option<&MagicOverlayHandle>,
) {
    if let Some(duration) = FocusAutoEvaluator::evaluate(config, focus_mode, current_app) {
        focus_mode.activate(duration, true);
        if let Some(overlay) = overlay {
            overlay.emit_focus_mode(true, true);
        }
        info!("Focus auto-activated: app={current_app}, duration={duration}m");
    }
}
