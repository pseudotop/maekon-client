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
                    // #6273: mask PII (Standard, fail-closed) like the export surface.
                    .map(|row| to_event_backup(row, &self.ctx.pii_sanitizer))
                    .collect::<Result<Vec<_>, _>>()?,
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
                    // #6273: mask PII (Standard, fail-closed) like the export surface.
                    .map(|row| to_frame_backup(row, &self.ctx.pii_sanitizer))
                    .collect::<Result<Vec<_>, _>>()?,
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
        // Non-failing observations. Kept out of `errors` because `success` is
        // `errors.is_empty()` and a relation pointing at a frame this device no
        // longer has is data-hygiene noise from #9721, not a failed restore.
        let mut notes: Vec<String> = Vec::new();
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

        // #9700: restore merges into the live table, so an archived tag id is
        // not authoritative — it may already belong to a different tag here, and
        // the archived name may already exist under a different id. Record where
        // each archived id actually landed so the relations below follow it.
        let mut tag_id_remap: std::collections::HashMap<i64, i64> =
            std::collections::HashMap::new();
        let mut frame_id_remap: std::collections::HashMap<i64, i64> =
            std::collections::HashMap::new();
        let mut skipped_relations_by_tag: std::collections::BTreeMap<i64, u64> =
            std::collections::BTreeMap::new();
        let mut skipped_relations_by_frame: std::collections::BTreeMap<i64, u64> =
            std::collections::BTreeMap::new();

        if let Some(tags) = &archive.tags {
            for tag in tags {
                match self
                    .ctx
                    .storage
                    .upsert_backup_tag(tag.id, &tag.name, &tag.color, &tag.created_at)
                    .await
                {
                    Ok(Some(effective_id)) => {
                        // A malformed archive can list the same id twice; the
                        // second entry would silently steal the first's
                        // relations, which is the mis-attachment this whole
                        // change exists to prevent.
                        if tag_id_remap.insert(tag.id, effective_id).is_some() {
                            errors.push(format!(
                                "Archive lists tag id {} more than once; its relations may be mis-attached",
                                tag.id
                            ));
                        }
                        restored.tags += 1;
                    }
                    // The erase barrier skipped the write — no row exists, so
                    // there is no id to remap onto.
                    Ok(None) => errors.push(format!(
                        "Tag '{}' was not restored (data deletion in progress)",
                        tag.name
                    )),
                    Err(error) => {
                        errors.push(format!("Failed to restore tag '{}': {error}", tag.name))
                    }
                }
            }
        }

        // #9708: frames must land before the relations that reference them, so
        // `frame_id` can be remapped the same way `tag_id` is. `frames.id` is
        // AUTOINCREMENT, so on a device that has been capturing, nearly every
        // archived id is already taken.
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
                    Ok(Some(effective_id)) => {
                        if frame_id_remap.insert(frame.id, effective_id).is_some() {
                            errors.push(format!(
                                "Archive lists frame id {} more than once; its relations may be mis-attached",
                                frame.id
                            ));
                        }
                        restored.frames += 1;
                    }
                    Ok(None) => errors.push(format!(
                        "Frame {} was not restored (data deletion in progress)",
                        frame.id
                    )),
                    Err(error) => errors.push(format!("Failed to restore frame: {error}")),
                }
            }
        }

        if let Some(frame_tags) = &archive.frame_tags {
            for frame_tag in frame_tags {
                // A relation whose tag is missing from the archive (or whose tag
                // failed to restore) has nowhere valid to point. Writing it
                // anyway used to create a dangling row that no error surfaced,
                // because foreign keys are not enforced.
                let Some(&tag_id) = tag_id_remap.get(&frame_tag.tag_id) else {
                    // Aggregate by tag: an archive missing one tag would
                    // otherwise emit one near-identical string per relation,
                    // thousands of them in a single JSON response.
                    skipped_relations_by_tag
                        .entry(frame_tag.tag_id)
                        .and_modify(|count| *count += 1)
                        .or_insert(1u64);
                    continue;
                };
                // Unlike tags — which the export always emits together with
                // `frame_tags` under one `include_tags` flag — frames are
                // governed by a SEPARATE `include_frames` flag. So an archive
                // legitimately carries relations without any frames, and those
                // ids refer to frames already on this device. Pass them through
                // in that case; only arbitrate when the archive brought frames
                // of its own and the ids may have moved.
                // The archive brought no frames: this id refers to a frame
                // already on THIS device, so pass it through. The write itself
                // verifies the frame exists (atomically, in the same statement),
                // so no separate check is needed here.
                let frame_id = match frame_id_remap.get(&frame_tag.frame_id) {
                    Some(&remapped) => remapped,
                    None if archive.frames.is_none() => frame_tag.frame_id,
                    None => {
                        skipped_relations_by_frame
                            .entry(frame_tag.frame_id)
                            .and_modify(|count| *count += 1)
                            .or_insert(1u64);
                        continue;
                    }
                };
                match self
                    .ctx
                    .storage
                    .upsert_backup_frame_tag(frame_id, tag_id, &frame_tag.created_at)
                    .await
                {
                    Ok(true) => restored.frame_tags += 1,
                    // The frame is not on this device — the guarded insert
                    // declined rather than writing a dangling row.
                    Ok(false) => {
                        skipped_relations_by_frame
                            .entry(frame_tag.frame_id)
                            .and_modify(|count| *count += 1)
                            .or_insert(1u64);
                    }
                    Err(error) => {
                        errors.push(format!("Failed to restore frame-tag relation: {error}"))
                    }
                }
            }
        }

        // Tag axis stays an ERROR, unlike the frame axis. The export emits
        // `tags` and `frame_tags` together under one `include_tags` flag, so a
        // self-produced archive can never reference a tag it does not carry —
        // that shape means a corrupt or hand-edited archive, which #9700
        // deliberately surfaces as a failure.
        //
        // The frame axis stays a NOTE for two reasons that both survive #9721:
        // a cross-device archive legitimately references frames this device
        // never had, and installs that ran retention before #9721 still carry
        // orphaned relations from that era.
        for (tag_id, count) in &skipped_relations_by_tag {
            errors.push(format!(
                "Skipped {count} frame-tag relation(s): tag {tag_id} is not in the archive"
            ));
        }

        for (frame_id, count) in &skipped_relations_by_frame {
            notes.push(format!(
                "Skipped {count} frame-tag relation(s): frame {frame_id} is not on this device"
            ));
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
                    Ok(true) => restored.events += 1,
                    // The erase barrier skipped the write — nothing landed.
                    Ok(false) => errors.push(format!(
                        "Event {} was not restored (data deletion in progress)",
                        event.event_id
                    )),
                    Err(error) => errors.push(format!("Failed to restore event: {error}")),
                }
            }
        }

        Ok(assemble_restore_result(restored, errors, notes))
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
