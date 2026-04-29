use axum::{
    extract::{Query, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use oneshim_api_contracts::backup::{BackupArchive, BackupQuery, RestoreResult};
#[cfg(test)]
use oneshim_api_contracts::backup::{BackupIncludes, BackupMetadata, RestoredCounts, TagBackup};

use crate::error::ApiError;
use crate::services::backup_service::{BackupCommandService, BackupDownload, BackupQueryService};
use crate::services::web_contexts::BackupWebContext;

pub async fn create_backup(
    State(context): State<BackupWebContext>,
    Query(params): Query<BackupQuery>,
) -> Result<Response, ApiError> {
    let BackupDownload { filename, body } =
        BackupQueryService::new(context).create_backup_download(&params)?;

    Ok((
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response())
}

pub async fn restore_backup(
    State(context): State<BackupWebContext>,
    Json(archive): Json<BackupArchive>,
) -> Result<Json<RestoreResult>, ApiError> {
    Ok(Json(
        BackupCommandService::new(context).restore_backup(&archive)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_query_defaults() {
        let query: BackupQuery = serde_json::from_str("{}").unwrap();
        assert!(query.include_settings);
        assert!(query.include_tags);
        assert!(!query.include_events);
        assert!(!query.include_frames);
    }

    #[test]
    fn backup_archive_serializes() {
        let archive = BackupArchive {
            metadata: BackupMetadata {
                version: "1.0".to_string(),
                created_at: "2024-01-30T10:00:00Z".to_string(),
                app_version: "0.1.0".to_string(),
                includes: BackupIncludes {
                    settings: true,
                    tags: true,
                    events: false,
                    frames: false,
                },
            },
            settings: None,
            tags: Some(vec![TagBackup {
                id: 1,
                name: "Work".to_string(),
                color: "#3b82f6".to_string(),
                created_at: "2024-01-30T10:00:00Z".to_string(),
            }]),
            frame_tags: None,
            events: None,
            frames: None,
        };

        let json = serde_json::to_string(&archive).unwrap();
        assert!(json.contains("\"version\":\"1.0\""));
        assert!(json.contains("\"Work\""));
    }

    #[test]
    fn restore_result_serializes() {
        let result = RestoreResult {
            success: true,
            restored: RestoredCounts {
                settings: true,
                tags: 5,
                frame_tags: 10,
                events: 0,
                frames: 0,
            },
            errors: vec![],
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"tags\":5"));
    }
}
