use chrono::Utc;
use oneshim_api_contracts::backup::{
    BackupArchive, BackupIncludes, BackupMetadata, EventBackup, FrameBackup, FrameTagBackup,
    RestoreResult, RestoredCounts, SettingsBackup, TagBackup,
};
use oneshim_core::models::storage_records::{
    EventExportRecord, FrameExportRecord, FrameTagLinkRecord, TagRecord,
};

use crate::services::web_contexts::BackupWebContext;

pub(crate) fn new_backup_archive(includes: BackupIncludes) -> BackupArchive {
    BackupArchive {
        metadata: BackupMetadata {
            version: "1.0".to_string(),
            created_at: Utc::now().to_rfc3339(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            includes,
        },
        settings: None,
        tags: None,
        frame_tags: None,
        events: None,
        frames: None,
    }
}

pub(crate) fn backup_filename() -> String {
    let now = Utc::now().format("%Y%m%d_%H%M%S");
    format!("oneshim_backup_{now}.json")
}

pub(crate) fn backup_settings_from_context(context: &BackupWebContext) -> SettingsBackup {
    if let Some(ref config_manager) = context.config_manager {
        let config = config_manager.get();
        SettingsBackup {
            capture_enabled: config.vision.capture_enabled,
            capture_interval_secs: (config.vision.capture_throttle_ms / 1000).max(1),
            idle_threshold_secs: config.monitor.idle_threshold_secs,
            metrics_interval_secs: config.monitor.poll_interval_ms / 1000,
            web_port: config.web.port,
            notification_enabled: config.notification.enabled,
            idle_notification_mins: config.notification.idle_notification_mins as u64,
            long_session_notification_mins: config.notification.long_session_mins as u64,
            high_usage_threshold_percent: config.notification.high_usage_threshold as u8,
        }
    } else {
        SettingsBackup {
            capture_enabled: true,
            capture_interval_secs: 60,
            idle_threshold_secs: 300,
            metrics_interval_secs: 5,
            web_port: oneshim_core::config::DEFAULT_WEB_PORT,
            notification_enabled: true,
            idle_notification_mins: 30,
            long_session_notification_mins: 60,
            high_usage_threshold_percent: 90,
        }
    }
}

pub(crate) fn to_tag_backup(tag: TagRecord) -> TagBackup {
    TagBackup {
        id: tag.id,
        name: tag.name,
        color: tag.color,
        created_at: tag.created_at,
    }
}

pub(crate) fn to_frame_tag_backup(frame_tag: FrameTagLinkRecord) -> FrameTagBackup {
    FrameTagBackup {
        frame_id: frame_tag.frame_id,
        tag_id: frame_tag.tag_id,
        created_at: frame_tag.created_at,
    }
}

pub(crate) fn to_event_backup(event: EventExportRecord) -> EventBackup {
    EventBackup {
        event_id: event.event_id,
        event_type: event.event_type,
        timestamp: event.timestamp,
        app_name: event.app_name,
        window_title: event.window_title,
    }
}

pub(crate) fn to_frame_backup(frame: FrameExportRecord) -> FrameBackup {
    FrameBackup {
        id: frame.id,
        timestamp: frame.timestamp,
        trigger_type: frame.trigger_type,
        app_name: frame.app_name,
        window_title: frame.window_title,
        importance: frame.importance,
        width: frame.resolution_w as i32,
        height: frame.resolution_h as i32,
        ocr_text: frame.ocr_text,
    }
}

pub(crate) fn empty_restore_counts() -> RestoredCounts {
    RestoredCounts {
        settings: false,
        tags: 0,
        frame_tags: 0,
        events: 0,
        frames: 0,
    }
}

pub(crate) fn assemble_restore_result(
    restored: RestoredCounts,
    errors: Vec<String>,
) -> RestoreResult {
    RestoreResult {
        success: errors.is_empty(),
        restored,
        errors,
    }
}
