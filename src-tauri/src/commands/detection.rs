use serde::Serialize;
use tauri::State;
use tracing::{info, warn};

use crate::ipc_error::IpcError;
use crate::runtime_state::DetectionRuntimeState;

#[derive(Debug, Serialize)]
pub struct ToggleDetectionResponse {
    pub active: bool,
}

#[tauri::command]
pub async fn toggle_detection_overlay(
    active: bool,
    state: State<'_, DetectionRuntimeState>,
) -> Result<ToggleDetectionResponse, IpcError> {
    state.set_active(active);

    if active {
        info!("detection overlay activated — running scene analysis");
        if let Some(overlay) = state.overlay() {
            overlay.set_interactive(true);
        }
        spawn_detection_analysis_from_state(&state);
    } else {
        info!("detection overlay deactivated");
        if let Some(overlay) = state.overlay() {
            overlay.clear_detection_scene().await;
            overlay.set_interactive(false);
        }
    }

    Ok(ToggleDetectionResponse { active })
}

#[tauri::command]
pub async fn refresh_detection_overlay(
    state: State<'_, DetectionRuntimeState>,
) -> Result<(), IpcError> {
    if !state.is_active() {
        // Precondition violation: caller requested an operation that depends
        // on runtime state not yet established. validation.invalid_arguments
        // surfaces this as "the request was malformed given current state"
        // — frontend can branch on this code to re-enable the overlay first.
        return Err(IpcError::new(
            "validation.invalid_arguments",
            "detection overlay is not active",
        ));
    }
    info!("detection overlay manual refresh");
    spawn_detection_analysis_from_state(&state);
    Ok(())
}

/// Helper for global shortcut handlers and IPC commands.
/// Clones only the detection-scoped resources needed for scene analysis.
pub fn spawn_detection_analysis_from_state(state: &DetectionRuntimeState) {
    let finder = match state.scene_finder() {
        Some(finder) => finder,
        None => {
            warn!("scene_finder not configured — cannot run detection");
            return;
        }
    };

    let overlay = match state.overlay() {
        Some(overlay) => overlay,
        None => {
            warn!("magic_overlay not available — cannot show detection");
            return;
        }
    };

    tokio::spawn(async move {
        match finder.analyze_scene(None, None).await {
            Ok(scene) => {
                overlay.emit_detection_scene(&scene).await;
            }
            Err(e) => {
                warn!("detection scene analysis failed: {e}");
            }
        }
    });
}
