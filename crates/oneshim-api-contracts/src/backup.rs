use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct BackupQuery {
    #[serde(default = "default_true")]
    pub include_settings: bool,
    #[serde(default = "default_true")]
    pub include_tags: bool,
    #[serde(default)]
    pub include_events: bool,
    #[serde(default)]
    pub include_frames: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupMetadata {
    pub version: String,
    pub created_at: String,
    pub app_version: String,
    pub includes: BackupIncludes,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupIncludes {
    pub settings: bool,
    pub tags: bool,
    pub events: bool,
    pub frames: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagBackup {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FrameTagBackup {
    pub frame_id: i64,
    pub tag_id: i64,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingsBackup {
    pub capture_enabled: bool,
    pub capture_interval_secs: u64,
    pub idle_threshold_secs: u64,
    pub metrics_interval_secs: u64,
    pub web_port: u16,
    pub notification_enabled: bool,
    pub idle_notification_mins: u64,
    pub long_session_notification_mins: u64,
    pub high_usage_threshold_percent: u8,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EventBackup {
    pub event_id: String,
    pub event_type: String,
    pub timestamp: String,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FrameBackup {
    pub id: i64,
    pub timestamp: String,
    pub trigger_type: String,
    pub app_name: String,
    pub window_title: String,
    pub importance: f32,
    pub width: i32,
    pub height: i32,
    pub ocr_text: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BackupArchive {
    pub metadata: BackupMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<SettingsBackup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<TagBackup>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_tags: Option<Vec<FrameTagBackup>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<EventBackup>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<Vec<FrameBackup>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_backup_metadata() {
        let original = BackupMetadata {
            version: "1.0".to_string(),
            created_at: "2026-04-11T00:00:00Z".to_string(),
            app_version: "0.4.33".to_string(),
            includes: BackupIncludes {
                settings: true,
                tags: true,
                events: false,
                frames: false,
            },
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: BackupMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_tag_backup() {
        let original = TagBackup {
            id: 42,
            name: "focus".to_string(),
            color: "#ff5733".to_string(),
            created_at: "2026-01-01T10:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: TagBackup = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_settings_backup() {
        let original = SettingsBackup {
            capture_enabled: true,
            capture_interval_secs: 30,
            idle_threshold_secs: 300,
            metrics_interval_secs: 5,
            web_port: 10090,
            notification_enabled: true,
            idle_notification_mins: 30,
            long_session_notification_mins: 90,
            high_usage_threshold_percent: 80,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: SettingsBackup = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_backup_includes_all_false() {
        let original = BackupIncludes {
            settings: false,
            tags: false,
            events: false,
            frames: false,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: BackupIncludes = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }
}

#[derive(Debug, Serialize)]
pub struct RestoreResult {
    pub success: bool,
    pub restored: RestoredCounts,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RestoredCounts {
    pub settings: bool,
    pub tags: u64,
    pub frame_tags: u64,
    pub events: u64,
    pub frames: u64,
}
