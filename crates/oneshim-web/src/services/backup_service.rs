use oneshim_api_contracts::backup::{
    BackupArchive, BackupIncludes, BackupQuery, RestoreResult, SettingsBackup,
};

use crate::error::ApiError;
use crate::services::backup_assembler::{
    assemble_restore_result, backup_filename, backup_settings_from_context, empty_restore_counts,
    new_backup_archive, to_event_backup, to_frame_backup, to_frame_tag_backup, to_tag_backup,
};
use crate::services::web_contexts::BackupWebContext;

const BACKUP_RANGE_START: &str = "0000-01-01T00:00:00Z";
const BACKUP_RANGE_END: &str = "9999-12-31T23:59:59Z";

pub struct BackupDownload {
    pub filename: String,
    pub body: String,
}

#[derive(Clone)]
pub struct BackupQueryService {
    ctx: BackupWebContext,
}

impl BackupQueryService {
    pub fn new(ctx: BackupWebContext) -> Self {
        Self { ctx }
    }

    pub fn create_backup_download(&self, params: &BackupQuery) -> Result<BackupDownload, ApiError> {
        let archive = self.create_backup_archive(params)?;
        let body = serde_json::to_string_pretty(&archive)
            .map_err(|error| ApiError::Internal(format!("JSON serialization failed: {error}")))?;

        Ok(BackupDownload {
            filename: backup_filename(),
            body,
        })
    }

    fn create_backup_archive(&self, params: &BackupQuery) -> Result<BackupArchive, ApiError> {
        let mut archive = new_backup_archive(BackupIncludes {
            settings: params.include_settings,
            tags: params.include_tags,
            events: params.include_events,
            frames: params.include_frames,
        });

        if params.include_settings {
            archive.settings = Some(backup_settings_from_context(&self.ctx));
        }

        if params.include_tags {
            archive.tags = Some(
                self.ctx
                    .storage
                    .list_backup_tags()
                    .map_err(|error| ApiError::Internal(error.to_string()))?
                    .into_iter()
                    .map(to_tag_backup)
                    .collect(),
            );
            archive.frame_tags = Some(
                self.ctx
                    .storage
                    .list_backup_frame_tags()
                    .map_err(|error| ApiError::Internal(error.to_string()))?
                    .into_iter()
                    .map(to_frame_tag_backup)
                    .collect(),
            );
        }

        if params.include_events {
            archive.events = Some(
                self.ctx
                    .storage
                    .list_event_exports(BACKUP_RANGE_START, BACKUP_RANGE_END)
                    .map_err(|error| ApiError::Internal(error.to_string()))?
                    .into_iter()
                    .map(to_event_backup)
                    .collect(),
            );
        }

        if params.include_frames {
            archive.frames = Some(
                self.ctx
                    .storage
                    .list_frame_exports(BACKUP_RANGE_START, BACKUP_RANGE_END)
                    .map_err(|error| ApiError::Internal(error.to_string()))?
                    .into_iter()
                    .map(to_frame_backup)
                    .collect(),
            );
        }

        Ok(archive)
    }
}

#[derive(Clone)]
pub struct BackupCommandService {
    ctx: BackupWebContext,
}

impl BackupCommandService {
    pub fn new(ctx: BackupWebContext) -> Self {
        Self { ctx }
    }

    pub fn restore_backup(&self, archive: &BackupArchive) -> Result<RestoreResult, ApiError> {
        let mut errors = Vec::new();
        let mut restored = empty_restore_counts();

        if let Some(settings) = &archive.settings {
            match restore_settings_to_context(&self.ctx, settings) {
                Ok(()) => restored.settings = true,
                Err(error) => errors.push(format!("Failed to restore settings: {error}")),
            }
        }

        if let Some(tags) = &archive.tags {
            for tag in tags {
                match self.ctx.storage.upsert_backup_tag(
                    tag.id,
                    &tag.name,
                    &tag.color,
                    &tag.created_at,
                ) {
                    Ok(()) => restored.tags += 1,
                    Err(error) => {
                        errors.push(format!("Failed to restore tag '{}': {error}", tag.name))
                    }
                }
            }
        }

        if let Some(frame_tags) = &archive.frame_tags {
            for frame_tag in frame_tags {
                match self.ctx.storage.upsert_backup_frame_tag(
                    frame_tag.frame_id,
                    frame_tag.tag_id,
                    &frame_tag.created_at,
                ) {
                    Ok(()) => restored.frame_tags += 1,
                    Err(error) => {
                        errors.push(format!("Failed to restore frame-tag relation: {error}"))
                    }
                }
            }
        }

        if let Some(events) = &archive.events {
            for event in events {
                match self.ctx.storage.upsert_backup_event(
                    &event.event_id,
                    &event.event_type,
                    &event.timestamp,
                    event.app_name.as_deref(),
                    event.window_title.as_deref(),
                ) {
                    Ok(()) => restored.events += 1,
                    Err(error) => errors.push(format!("Failed to restore event: {error}")),
                }
            }
        }

        if let Some(frames) = &archive.frames {
            for frame in frames {
                match self.ctx.storage.upsert_backup_frame(
                    frame.id,
                    &frame.timestamp,
                    &frame.trigger_type,
                    &frame.app_name,
                    &frame.window_title,
                    frame.importance,
                    frame.width,
                    frame.height,
                    frame.ocr_text.as_deref(),
                ) {
                    Ok(()) => restored.frames += 1,
                    Err(error) => errors.push(format!("Failed to restore frame: {error}")),
                }
            }
        }

        Ok(assemble_restore_result(restored, errors))
    }
}

fn restore_settings_to_context(
    context: &BackupWebContext,
    settings: &SettingsBackup,
) -> Result<(), ApiError> {
    let config_manager = context
        .config_manager
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Cannot restore without config manager".to_string()))?;

    config_manager
        .update_with(|config| {
            config.vision.capture_enabled = settings.capture_enabled;
            config.vision.capture_throttle_ms = settings.capture_interval_secs.saturating_mul(1000);
            config.monitor.idle_threshold_secs = settings.idle_threshold_secs;
            config.monitor.poll_interval_ms = settings.metrics_interval_secs.saturating_mul(1000);
            config.web.port = settings.web_port;
            config.notification.enabled = settings.notification_enabled;
            config.notification.idle_notification_mins = settings.idle_notification_mins as u32;
            config.notification.long_session_mins = settings.long_session_notification_mins as u32;
            config.notification.high_usage_threshold = settings.high_usage_threshold_percent as u32;
            Ok(())
        })
        .map_err(ApiError::from)?;

    Ok(())
}
