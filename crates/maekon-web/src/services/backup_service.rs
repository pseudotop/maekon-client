use maekon_api_contracts::backup::{
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

    pub async fn create_backup_download(
        &self,
        params: &BackupQuery,
    ) -> Result<BackupDownload, ApiError> {
        let archive = self.create_backup_archive(params).await?;
        let body = serde_json::to_string_pretty(&archive)
            .map_err(|error| ApiError::Internal(format!("JSON serialization failed: {error}")))?;

        Ok(BackupDownload {
            filename: backup_filename(),
            body,
        })
    }

    async fn create_backup_archive(&self, params: &BackupQuery) -> Result<BackupArchive, ApiError> {
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
                    .await
                    .map_err(|error| ApiError::Internal(error.to_string()))?
                    .into_iter()
                    .map(to_tag_backup)
                    .collect(),
            );
            archive.frame_tags = Some(
                self.ctx
                    .storage
                    .list_backup_frame_tags()
                    .await
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
                    .await
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
                    .await
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

    pub async fn restore_backup(&self, archive: &BackupArchive) -> Result<RestoreResult, ApiError> {
        let mut errors = Vec::new();
        let mut restored = empty_restore_counts();

        if let Some(settings) = &archive.settings {
            match restore_settings_to_context(self.ctx.config_manager.as_ref(), settings) {
                Ok(locked_fields) => {
                    restored.settings = true;
                    // The write chokepoint silently clamps admin-locked fields;
                    // surface them so the restore is not reported as a clean
                    // success when a managed value overrode the backup (#4832).
                    if !locked_fields.is_empty() {
                        errors.push(format!(
                            "Some settings are locked by your administrator and were kept at their managed values: {}",
                            locked_fields.join(", ")
                        ));
                    }
                }
                Err(error) => errors.push(format!("Failed to restore settings: {error}")),
            }
        }

        if let Some(tags) = &archive.tags {
            for tag in tags {
                match self
                    .ctx
                    .storage
                    .upsert_backup_tag(tag.id, &tag.name, &tag.color, &tag.created_at)
                    .await
                {
                    Ok(()) => restored.tags += 1,
                    Err(error) => {
                        errors.push(format!("Failed to restore tag '{}': {error}", tag.name))
                    }
                }
            }
        }

        if let Some(frame_tags) = &archive.frame_tags {
            for frame_tag in frame_tags {
                match self
                    .ctx
                    .storage
                    .upsert_backup_frame_tag(
                        frame_tag.frame_id,
                        frame_tag.tag_id,
                        &frame_tag.created_at,
                    )
                    .await
                {
                    Ok(()) => restored.frame_tags += 1,
                    Err(error) => {
                        errors.push(format!("Failed to restore frame-tag relation: {error}"))
                    }
                }
            }
        }

        if let Some(events) = &archive.events {
            for event in events {
                match self
                    .ctx
                    .storage
                    .upsert_backup_event(
                        &event.event_id,
                        &event.event_type,
                        &event.timestamp,
                        event.app_name.as_deref(),
                        event.window_title.as_deref(),
                    )
                    .await
                {
                    Ok(()) => restored.events += 1,
                    Err(error) => errors.push(format!("Failed to restore event: {error}")),
                }
            }
        }

        if let Some(frames) = &archive.frames {
            for frame in frames {
                match self
                    .ctx
                    .storage
                    .upsert_backup_frame(
                        frame.id,
                        &frame.timestamp,
                        &frame.trigger_type,
                        &frame.app_name,
                        &frame.window_title,
                        frame.importance,
                        frame.width,
                        frame.height,
                        frame.ocr_text.as_deref(),
                    )
                    .await
                {
                    Ok(()) => restored.frames += 1,
                    Err(error) => errors.push(format!("Failed to restore frame: {error}")),
                }
            }
        }

        Ok(assemble_restore_result(restored, errors))
    }
}

/// Apply the backup's settings fields onto `config`. Shared by the violation
/// pre-check and the authoritative write so the two never drift.
fn apply_backup_settings(config: &mut maekon_core::config::AppConfig, settings: &SettingsBackup) {
    config.vision.capture_enabled = settings.capture_enabled;
    config.vision.capture_throttle_ms = settings.capture_interval_secs.saturating_mul(1000);
    config.monitor.idle_threshold_secs = settings.idle_threshold_secs;
    config.monitor.poll_interval_ms = settings.metrics_interval_secs.saturating_mul(1000);
    config.web.port = settings.web_port;
    config.notification.enabled = settings.notification_enabled;
    config.notification.idle_notification_mins = settings.idle_notification_mins as u32;
    config.notification.long_session_mins = settings.long_session_notification_mins as u32;
    config.notification.high_usage_threshold = settings.high_usage_threshold_percent as u32;
}

/// Restore the settings section of a backup.
///
/// Returns the dotted-path identities of any managed-locked fields the backup
/// tried to change: the write chokepoint clamps them to the admin value, and
/// the caller surfaces them so a restore isn't reported as a clean success when
/// a locked value silently overrode the backup (#4832).
fn restore_settings_to_context(
    config_manager: Option<&maekon_core::config_manager::ConfigManager>,
    settings: &SettingsBackup,
) -> Result<Vec<String>, ApiError> {
    let config_manager = config_manager
        .ok_or_else(|| ApiError::Internal("Cannot restore without config manager".to_string()))?;

    // Detect locked-field violations on the candidate before the clamped write.
    let mut candidate = config_manager.get();
    apply_backup_settings(&mut candidate, settings);
    let locked_fields = config_manager.detect_managed_violations(&candidate);

    config_manager
        .update_with(|config| {
            apply_backup_settings(config, settings);
            Ok(())
        })
        .map_err(ApiError::from)?;

    Ok(locked_fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::{ManagedConfig, ManagedVision};
    use maekon_core::config_manager::ConfigManager;
    use tempfile::TempDir;

    fn backup_with_capture(capture_enabled: bool) -> SettingsBackup {
        SettingsBackup {
            capture_enabled,
            capture_interval_secs: 5,
            idle_threshold_secs: 300,
            metrics_interval_secs: 5,
            web_port: maekon_core::config::DEFAULT_WEB_PORT,
            notification_enabled: true,
            idle_notification_mins: 30,
            long_session_notification_mins: 60,
            high_usage_threshold_percent: 90,
        }
    }

    fn manager_locking_capture(dir: &std::path::Path, locked: bool) -> ConfigManager {
        let managed = ManagedConfig {
            vision: ManagedVision {
                capture_enabled: Some(locked),
            },
            ..Default::default()
        };
        let managed_path = dir.join("managed.json");
        std::fs::write(&managed_path, serde_json::to_string(&managed).unwrap()).unwrap();
        ConfigManager::with_paths(dir.join("config.json"), Some(managed_path)).unwrap()
    }

    #[test]
    fn restore_reports_locked_field_kept_at_managed_value() {
        // Admin locks vision.capture_enabled = false; a backup that turns it on
        // must be clamped AND reported (not a silent revert) — the third
        // interactive surface the pre-merge review flagged (#4832).
        let dir = TempDir::new().unwrap();
        let cm = manager_locking_capture(dir.path(), false);

        let locked = restore_settings_to_context(Some(&cm), &backup_with_capture(true))
            .expect("restore must succeed");

        assert_eq!(locked, vec!["vision.capture_enabled".to_string()]);
        // The locked value held (clamped to the managed false).
        assert!(!cm.get().vision.capture_enabled);
    }

    #[test]
    fn restore_reports_nothing_when_value_complies() {
        let dir = TempDir::new().unwrap();
        // Lock capture = true; a backup that also wants true is compliant.
        let cm = manager_locking_capture(dir.path(), true);

        let locked = restore_settings_to_context(Some(&cm), &backup_with_capture(true))
            .expect("restore must succeed");

        assert!(locked.is_empty());
        assert!(cm.get().vision.capture_enabled);
    }
}
