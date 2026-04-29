use axum::extract::{Path, State};
use axum::Json;
use chrono::Utc;
use oneshim_api_contracts::annotations::CreateAnnotationRequest;
use oneshim_core::models::annotation::FrameAnnotation;

use crate::error::ApiError;
use crate::services::web_contexts::StorageWebContext;

/// GET /api/frames/{frame_id}/annotations
pub async fn list_annotations(
    State(context): State<StorageWebContext>,
    Path(frame_id): Path<i64>,
) -> Result<Json<Vec<FrameAnnotation>>, ApiError> {
    let annotations = context.storage.list_annotations(frame_id)?;
    Ok(Json(annotations))
}

/// POST /api/frames/{frame_id}/annotations
pub async fn create_annotation(
    State(context): State<StorageWebContext>,
    Path(frame_id): Path<i64>,
    Json(req): Json<CreateAnnotationRequest>,
) -> Result<Json<FrameAnnotation>, ApiError> {
    let annotation = FrameAnnotation {
        annotation_id: uuid::Uuid::new_v4().to_string(),
        frame_id,
        annotation_type: req.annotation_type,
        x: req.x,
        y: req.y,
        width: req.width,
        height: req.height,
        color: req.color,
        text: req.text,
        created_at: Utc::now(),
    };

    context.storage.save_annotation(&annotation)?;
    Ok(Json(annotation))
}

/// DELETE /api/frames/{frame_id}/annotations/{annotation_id}
pub async fn delete_annotation(
    State(context): State<StorageWebContext>,
    Path((_frame_id, annotation_id)): Path<(i64, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    context.storage.delete_annotation(&annotation_id)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::annotation::AnnotationType;

    #[test]
    fn create_annotation_request_deserializes() {
        let json = r##"{
            "annotation_type": "Highlight",
            "x": 10.0,
            "y": 20.0,
            "width": 100.0,
            "height": 50.0,
            "color": "#ff0000",
            "text": "Important"
        }"##;
        let req: CreateAnnotationRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.annotation_type, AnnotationType::Highlight);
        assert_eq!(req.x, 10.0);
        assert_eq!(req.color, Some("#ff0000".to_string()));
    }

    #[test]
    fn create_annotation_request_minimal() {
        let json = r##"{
            "annotation_type": "Arrow",
            "x": 5.0,
            "y": 15.0
        }"##;
        let req: CreateAnnotationRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.annotation_type, AnnotationType::Arrow);
        assert_eq!(req.width, 0.0);
        assert_eq!(req.height, 0.0);
        assert!(req.color.is_none());
        assert!(req.text.is_none());
    }
}
