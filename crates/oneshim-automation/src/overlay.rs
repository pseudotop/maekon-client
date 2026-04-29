use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

use oneshim_core::error::CoreError;
use oneshim_core::models::gui::{HighlightHandle, HighlightRequest};
use oneshim_core::models::ui_scene::UiScene;
use oneshim_core::ports::overlay_driver::OverlayDriver;

pub struct NoOpOverlayDriver;

#[async_trait]
impl OverlayDriver for NoOpOverlayDriver {
    async fn show_highlights(&self, req: HighlightRequest) -> Result<HighlightHandle, CoreError> {
        tracing::info!(
            session_id = %req.session_id,
            scene_id = %req.scene_id,
            target_count = req.targets.len(),
            "NoOpOverlayDriver accepted highlight request"
        );

        Ok(HighlightHandle {
            handle_id: Uuid::new_v4().to_string(),
            rendered_at: Utc::now(),
            target_count: req.targets.len(),
        })
    }

    async fn clear_highlights(&self, handle_id: &str) -> Result<(), CoreError> {
        tracing::debug!(handle_id, "NoOpOverlayDriver cleared highlight handle");
        Ok(())
    }

    async fn show_detection(&self, scene: &UiScene) -> Result<(), CoreError> {
        tracing::debug!(
            scene_id = %scene.scene_id,
            element_count = scene.elements.len(),
            "NoOpOverlayDriver accepted detection scene"
        );
        Ok(())
    }

    async fn clear_detection(&self) -> Result<(), CoreError> {
        tracing::debug!("NoOpOverlayDriver cleared detection overlay");
        Ok(())
    }
}
