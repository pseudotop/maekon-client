//! UI element discovery port — defines the contract for finding clickable
//! elements and analyzing UI scenes for the automation pipeline.
//! Implemented by accessibility and vision adapters in `oneshim-vision`.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::intent::{ElementBounds, UiElement};
use crate::models::ui_scene::UiScene;

/// # Errors
/// - Accessibility adapter path: `CoreError::PermissionDenied`
///   (wire: `permission.permission_denied`) if OS accessibility
///   permission is missing; `CoreError::ElementNotFound`
///   (wire: `ui.element_missing`) if requested element is not present.
/// - Scene-analysis adapters may additionally emit `CoreError::OcrError`
///   (wire: `provider.ocr_failed`) and `CoreError::Analysis`
///   (wire: `provider.analysis_failed`) propagated from underlying
///   OCR/LLM providers.
/// - Default `analyze_scene` / `analyze_scene_from_image` impls emit
///   `CoreError::Internal` (`internal.generic`) with "does not support
///   scene analysis" / "does not support direct image scene analysis"
///   messages. Caller `oneshim-web::automation_service::scene::analyze_scene`
///   pattern-matches BOTH `Internal` AND `Config` variants where message
///   contains "Scene analyzer" (Config.Missing from iter-100 controller
///   gate), "scene analysis is not supported", or "direct image scene
///   analysis", and routes to HTTP 400 BadRequest. See iter-101/104
///   cascading fix. NOTE: the first default's message ("does not support
///   scene analysis") doesn't match any of the three substrings; if a
///   future adapter leaves the default in place, its Err would fall
///   through to ApiError::Internal. All current production adapters
///   override both defaults, so this gap is latent.
#[async_trait]
pub trait ElementFinder: Send + Sync {
    async fn find_element(
        &self,
        text: Option<&str>,
        role: Option<&str>,
        region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError>;

    async fn analyze_scene(
        &self,
        _app_name: Option<&str>,
        _screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: format!(
                "ElementFinder '{}' does not support scene analysis",
                self.name()
            ),
        })
    }

    async fn analyze_scene_from_image(
        &self,
        _image_data: Vec<u8>,
        _image_format: String,
        _app_name: Option<&str>,
        _screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: format!(
                "ElementFinder '{}' does not support direct image scene analysis",
                self.name()
            ),
        })
    }

    fn name(&self) -> &str;
}
