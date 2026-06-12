use maekon_api_contracts::tags::{CreateTagRequest, TagResponse, UpdateTagRequest};

use crate::error::ApiError;
use crate::services::tags_assembler::assemble_tag_response;
use crate::services::web_contexts::StorageWebContext;

#[derive(Clone)]
pub struct TagsQueryService {
    ctx: StorageWebContext,
}

impl TagsQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub async fn list_tags(&self) -> Result<Vec<TagResponse>, ApiError> {
        self.ctx
            .storage
            .get_all_tags()
            .await
            .map_err(ApiError::from)
            .map(|tags| tags.into_iter().map(assemble_tag_response).collect())
    }

    pub async fn get_tag(&self, tag_id: i64) -> Result<TagResponse, ApiError> {
        let tag = self
            .ctx
            .storage
            .get_tag(tag_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("Tag ID: {tag_id}")))?;

        Ok(assemble_tag_response(tag))
    }

    pub async fn get_frame_tags(&self, frame_id: i64) -> Result<Vec<TagResponse>, ApiError> {
        self.ctx
            .storage
            .get_tags_for_frame(frame_id)
            .await
            .map_err(ApiError::from)
            .map(|tags| tags.into_iter().map(assemble_tag_response).collect())
    }
}

#[derive(Clone)]
pub struct TagsCommandService {
    ctx: StorageWebContext,
}

impl TagsCommandService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub async fn create_tag(&self, request: &CreateTagRequest) -> Result<TagResponse, ApiError> {
        let color = request
            .color
            .clone()
            .unwrap_or_else(|| "#3b82f6".to_string());

        let tag = self.ctx.storage.create_tag(&request.name, &color).await?;
        Ok(assemble_tag_response(tag))
    }

    pub async fn update_tag(
        &self,
        tag_id: i64,
        request: &UpdateTagRequest,
    ) -> Result<TagResponse, ApiError> {
        let updated = self
            .ctx
            .storage
            .update_tag(tag_id, &request.name, &request.color)
            .await?;

        if !updated {
            return Err(ApiError::NotFound(format!("Tag ID: {tag_id}")));
        }

        TagsQueryService::new(self.ctx.clone())
            .get_tag(tag_id)
            .await
    }

    pub async fn delete_tag(&self, tag_id: i64) -> Result<serde_json::Value, ApiError> {
        let deleted = self.ctx.storage.delete_tag(tag_id).await?;

        if !deleted {
            return Err(ApiError::NotFound(format!("Tag ID: {tag_id}")));
        }

        Ok(serde_json::json!({ "message": "Tag deleted." }))
    }

    pub async fn add_tag_to_frame(
        &self,
        frame_id: i64,
        tag_id: i64,
    ) -> Result<serde_json::Value, ApiError> {
        self.ctx.storage.add_tag_to_frame(frame_id, tag_id).await?;
        Ok(serde_json::json!({ "message": "Tag added to frame." }))
    }

    pub async fn remove_tag_from_frame(
        &self,
        frame_id: i64,
        tag_id: i64,
    ) -> Result<serde_json::Value, ApiError> {
        let removed = self
            .ctx
            .storage
            .remove_tag_from_frame(frame_id, tag_id)
            .await?;

        if !removed {
            return Err(ApiError::NotFound(format!(
                "Tag {tag_id} is not attached to frame {frame_id}."
            )));
        }

        Ok(serde_json::json!({ "message": "Tag removed from frame." }))
    }
}
