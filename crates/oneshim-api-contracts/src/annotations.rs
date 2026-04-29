use oneshim_core::models::annotation::AnnotationType;
use serde::Deserialize;

/// Request body for `POST /api/frames/{frame_id}/annotations`.
#[derive(Debug, Deserialize)]
pub struct CreateAnnotationRequest {
    pub annotation_type: AnnotationType,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub width: f32,
    #[serde(default)]
    pub height: f32,
    pub color: Option<String>,
    pub text: Option<String>,
}
