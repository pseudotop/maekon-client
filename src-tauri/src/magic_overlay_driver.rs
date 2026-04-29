//! Bridge between the OverlayDriver port and the MagicOverlay Tauri WebView.
//!
//! Translates HighlightRequest into Tauri events consumed by the
//! FocusHighlight React component in the overlay window.

use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use oneshim_core::error::CoreError;
use oneshim_core::models::gui::{HighlightHandle, HighlightRequest};
use oneshim_core::models::ui_scene::UiScene;
use oneshim_core::ports::overlay_driver::OverlayDriver;

/// Serializable highlight data emitted to the overlay WebView.
#[derive(Debug, Clone, Serialize)]
struct FocusHighlightPayload {
    pub handle_id: String,
    pub targets: Vec<FocusTargetPayload>,
}

#[derive(Debug, Clone, Serialize)]
struct FocusTargetPayload {
    pub candidate_id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub color: String,
    pub label: Option<String>,
}

/// OverlayDriver implementation that bridges to the MagicOverlay Tauri WebView.
///
/// Emits `overlay:update-focus` and `overlay:clear-focus` events that the
/// FocusHighlight React component listens for.
pub struct MagicOverlayDriver {
    app_handle: AppHandle,
}

impl MagicOverlayDriver {
    pub fn new(app_handle: AppHandle) -> Self {
        Self { app_handle }
    }
}

#[async_trait]
impl OverlayDriver for MagicOverlayDriver {
    async fn show_highlights(&self, req: HighlightRequest) -> Result<HighlightHandle, CoreError> {
        let handle_id = Uuid::new_v4().to_string();
        let target_count = req.targets.len();

        let payload = FocusHighlightPayload {
            handle_id: handle_id.clone(),
            targets: req
                .targets
                .into_iter()
                .map(|t| FocusTargetPayload {
                    candidate_id: t.candidate_id,
                    x: t.bbox_abs.x,
                    y: t.bbox_abs.y,
                    width: t.bbox_abs.width,
                    height: t.bbox_abs.height,
                    color: t.color,
                    label: t.label,
                })
                .collect(),
        };

        self.app_handle
            .emit("overlay:update-focus", &payload)
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Failed to emit overlay:update-focus: {e}"),
            })?;

        tracing::debug!(
            handle_id = %handle_id,
            target_count,
            "MagicOverlayDriver: emitted focus highlights"
        );

        Ok(HighlightHandle {
            handle_id,
            rendered_at: Utc::now(),
            target_count,
        })
    }

    async fn clear_highlights(&self, handle_id: &str) -> Result<(), CoreError> {
        self.app_handle
            .emit("overlay:clear-focus", handle_id)
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Failed to emit overlay:clear-focus: {e}"),
            })?;

        tracing::debug!(handle_id, "MagicOverlayDriver: cleared highlights");
        Ok(())
    }

    async fn show_detection(&self, scene: &UiScene) -> Result<(), CoreError> {
        // Detection scene emission is handled by MagicOverlayHandle.emit_detection_scene(),
        // not by OverlayDriver. This stub exists for trait completeness.
        tracing::debug!(
            scene_id = %scene.scene_id,
            element_count = scene.elements.len(),
            "MagicOverlayDriver: detection scene (handled by MagicOverlayHandle)"
        );
        Ok(())
    }

    async fn clear_detection(&self) -> Result<(), CoreError> {
        tracing::debug!("MagicOverlayDriver: clear detection (handled by MagicOverlayHandle)");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_target_payload_serde() {
        let payload = FocusTargetPayload {
            candidate_id: "el-1".to_string(),
            x: 100,
            y: 200,
            width: 80,
            height: 30,
            color: "#3b82f6".to_string(),
            label: Some("Save".to_string()),
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        assert!(json.contains("\"x\":100"));
        assert!(json.contains("Save"));
    }

    #[test]
    fn focus_highlight_payload_serde() {
        let payload = FocusHighlightPayload {
            handle_id: "handle-1".to_string(),
            targets: vec![FocusTargetPayload {
                candidate_id: "el-1".to_string(),
                x: 10,
                y: 20,
                width: 100,
                height: 30,
                color: "#ef4444".to_string(),
                label: None,
            }],
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        assert!(json.contains("handle-1"));
        assert!(json.contains("\"width\":100"));
    }
}
