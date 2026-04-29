use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Duration;
use oneshim_api_contracts::frames::FrameResponse;
use std::path::{Path, PathBuf};

use crate::error::ApiError;
use crate::services::frames_assembler::assemble_frame_response;
use crate::services::web_contexts::StorageWebContext;
use oneshim_api_contracts::common::{PaginatedResponse, PaginationMeta, TimeRangeQuery};

#[derive(Clone)]
pub struct FramesQueryService {
    ctx: StorageWebContext,
}

impl FramesQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub fn get_frames(
        &self,
        params: &TimeRangeQuery,
    ) -> Result<PaginatedResponse<FrameResponse>, ApiError> {
        let window = params
            .to_time_window(Duration::hours(24))
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let limit = params.limit_or_default();
        let offset = params.offset_or_default();

        let min_importance = params.min_importance.unwrap_or(0.0) as f32;

        // Fetch all frames in range (no limit) so we can filter by importance first,
        // then paginate the filtered result for correct total count.
        // get_frames is out of plan scope (still takes DateTime<Utc>): decompose.
        let all_frames = self
            .ctx
            .storage
            .get_frames(window.start, window.end, usize::MAX)?;

        let filtered: Vec<_> = all_frames
            .into_iter()
            .filter(|f| f.importance >= min_importance)
            .collect();

        let total = filtered.len() as u64;

        let data: Vec<FrameResponse> = filtered
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(assemble_frame_response)
            .collect();

        let data: Vec<FrameResponse> = {
            let frame_ids: Vec<i64> = data.iter().map(|frame| frame.id).collect();
            if frame_ids.is_empty() {
                data
            } else {
                let tag_map = self
                    .ctx
                    .storage
                    .get_tag_ids_for_frames(&frame_ids)
                    .map_err(ApiError::from)?;

                data.into_iter()
                    .map(|mut frame| {
                        if let Some(tags) = tag_map.get(&frame.id) {
                            frame.tag_ids = tags.clone();
                        }
                        frame
                    })
                    .collect()
            }
        };

        let has_more = (offset + data.len()) < total as usize;

        Ok(PaginatedResponse {
            data,
            pagination: PaginationMeta {
                total,
                offset,
                limit,
                has_more,
            },
        })
    }

    pub async fn get_frame_image(&self, frame_id: i64) -> Response {
        let file_path = match self.ctx.storage.get_frame_file_path(frame_id) {
            Ok(Some(path)) => path,
            Ok(None) => {
                return ApiError::NotFound(format!("frame {frame_id} has no image"))
                    .into_response();
            }
            Err(error) => return ApiError::Internal(error.to_string()).into_response(),
        };

        let full_path = match resolve_frame_image_path(self.ctx.frames_dir.as_deref(), &file_path) {
            Ok(path) => path,
            Err(error) => return error.into_response(),
        };

        let data = if let Some(ref frame_storage) = self.ctx.frame_storage {
            match frame_storage.load_frame(Path::new(&file_path)).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    return ApiError::Internal(format!("frame load failure: {error}"))
                        .into_response();
                }
            }
        } else {
            match std::fs::read(&full_path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return ApiError::Internal(format!("file read failure: {error}"))
                        .into_response();
                }
            }
        };

        let content_type = mime_guess::from_path(&full_path)
            .first_or_octet_stream()
            .to_string();

        (StatusCode::OK, [(header::CONTENT_TYPE, content_type)], data).into_response()
    }
}

fn resolve_frame_image_path(
    frames_dir: Option<&Path>,
    file_path: &str,
) -> Result<PathBuf, ApiError> {
    if let Some(frames_dir) = frames_dir {
        let joined = frames_dir.join(file_path);
        return match joined.canonicalize() {
            Ok(canonical) => {
                let frames_canonical = frames_dir
                    .canonicalize()
                    .unwrap_or_else(|_| frames_dir.to_path_buf());
                if !canonical.starts_with(&frames_canonical) {
                    return Err(ApiError::BadRequest("Invalid file path".to_string()));
                }
                Ok(canonical)
            }
            Err(_) => Err(ApiError::NotFound(format!(
                "Image file not found: {file_path}"
            ))),
        };
    }

    Ok(PathBuf::from(file_path))
}
