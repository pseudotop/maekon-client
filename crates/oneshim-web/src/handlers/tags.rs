use axum::extract::{Path, State};
use axum::Json;
use oneshim_api_contracts::tags::{
    BatchTagRequest, BatchTagResponse, CreateTagRequest, TagResponse, UpdateTagRequest,
};

use crate::error::ApiError;
use crate::services::tags_service::{TagsCommandService, TagsQueryService};
use crate::services::web_contexts::StorageWebContext;

/// GET /api/tags
pub async fn list_tags(
    State(context): State<StorageWebContext>,
) -> Result<Json<Vec<TagResponse>>, ApiError> {
    Ok(Json(TagsQueryService::new(context).list_tags()?))
}

/// POST /api/tags
pub async fn create_tag(
    State(context): State<StorageWebContext>,
    Json(req): Json<CreateTagRequest>,
) -> Result<Json<TagResponse>, ApiError> {
    Ok(Json(TagsCommandService::new(context).create_tag(&req)?))
}

/// GET /api/tags/:id
pub async fn get_tag(
    State(context): State<StorageWebContext>,
    Path(tag_id): Path<i64>,
) -> Result<Json<TagResponse>, ApiError> {
    Ok(Json(TagsQueryService::new(context).get_tag(tag_id)?))
}

/// PUT /api/tags/:id
pub async fn update_tag(
    State(context): State<StorageWebContext>,
    Path(tag_id): Path<i64>,
    Json(req): Json<UpdateTagRequest>,
) -> Result<Json<TagResponse>, ApiError> {
    Ok(Json(
        TagsCommandService::new(context).update_tag(tag_id, &req)?,
    ))
}

/// DELETE /api/tags/:id
pub async fn delete_tag(
    State(context): State<StorageWebContext>,
    Path(tag_id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(TagsCommandService::new(context).delete_tag(tag_id)?))
}

/// GET /api/frames/:frame_id/tags
pub async fn get_frame_tags(
    State(context): State<StorageWebContext>,
    Path(frame_id): Path<i64>,
) -> Result<Json<Vec<TagResponse>>, ApiError> {
    Ok(Json(
        TagsQueryService::new(context).get_frame_tags(frame_id)?,
    ))
}

/// POST /api/frames/:frame_id/tags/:tag_id
pub async fn add_tag_to_frame(
    State(context): State<StorageWebContext>,
    Path((frame_id, tag_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        TagsCommandService::new(context).add_tag_to_frame(frame_id, tag_id)?,
    ))
}

/// DELETE /api/frames/:frame_id/tags/:tag_id
pub async fn remove_tag_from_frame(
    State(context): State<StorageWebContext>,
    Path((frame_id, tag_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        TagsCommandService::new(context).remove_tag_from_frame(frame_id, tag_id)?,
    ))
}

/// POST /api/frames/batch-tags
pub async fn batch_add_tag(
    State(context): State<StorageWebContext>,
    Json(req): Json<BatchTagRequest>,
) -> Result<Json<BatchTagResponse>, ApiError> {
    let mut tagged_count = 0u32;
    for frame_id in &req.frame_ids {
        match TagsCommandService::new(context.clone()).add_tag_to_frame(*frame_id, req.tag_id) {
            Ok(_) => tagged_count += 1,
            Err(e) => {
                tracing::warn!("batch tag: frame {} failed: {}", frame_id, e);
            }
        }
    }
    Ok(Json(BatchTagResponse { tagged_count }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_response_serializes() {
        let tag = TagResponse {
            id: 1,
            name: "important".to_string(),
            color: "#ef4444".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&tag).unwrap();
        assert!(json.contains("important"));
        assert!(json.contains("#ef4444"));
    }

    #[test]
    fn create_tag_request_deserializes() {
        let json = r##"{"name": "work", "color": "#3b82f6"}"##;
        let req: CreateTagRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "work");
        assert_eq!(req.color, Some("#3b82f6".to_string()));
    }

    #[test]
    fn create_tag_request_without_color() {
        let json = r##"{"name": "work"}"##;
        let req: CreateTagRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "work");
        assert!(req.color.is_none());
    }
}
